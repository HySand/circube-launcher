<template>
  <div class="h-full flex flex-col items-center justify-center bg-white animate-fade-in gap-5">
    <Spinner class="text-blue-700" />

    <p class="text-base text-gray-500 font-medium tracking-tight animate-pulse min-h-[1.5rem]">
      {{ statusText }}
    </p>

    <button v-if="showSwitchCdnButton" type="button" :disabled="isSwitchingCdn" @click="switchToChinaCdn"
            class="h-10 rounded-full bg-blue-600 px-5 text-[12px] font-black text-white shadow-sm transition active:scale-[0.98] disabled:opacity-50">
      {{ isSwitchingCdn ? '切换中...' : '切换下载源？' }}
    </button>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, onUnmounted } from "vue"
import { useRouter } from "vue-router"
import { useCacheStore } from '@/stores/cache'
import { invoke } from "@tauri-apps/api/core";
import { Spinner } from "@/components/ui/spinner"
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import { toast } from 'vue-sonner';

const router = useRouter()
const cache = useCacheStore()
const statusText = ref("正在启动启动器...");
const showSwitchCdnButton = ref(false);
const isSwitchingCdn = ref(false);
const currentSpeedText = ref("0KB/s");
const downloadProgress = ref<{ current: number; total: number; label: string } | null>(null);
let unlisten: UnlistenFn;
let unlistenSpeed: UnlistenFn;
const SPEED_SMOOTHING_ALPHA = 0.35;
const SPEED_ZERO_HOLD_MS = 2500;
let displayedSpeedBytesPerSec = 0;
let lastActiveSpeedBytesPerSec = 0;
let lastActiveSpeedAt = 0;

const fetchQuote = async () => {
  const response = await fetch("https://v1.hitokoto.cn/?c=d")
  const data = await response.json()
  cache.setQuote({ text: data.hitokoto, from: data.from })
}

interface UserInfo { uuid: string; name: string; accessToken: string; skinUrl: string; authType: string }
interface Config { javaPath: string; maxMemory: number; downloadSource: 'r2' | 'bitiful' }
interface JavaInfo { path: string; version: string }
interface DownloadSpeedPayload {
  averageBytesPerSec: number;
  currentBytesPerSec: number;
  lowSpeed: boolean;
  source: 'r2' | 'bitiful';
}

const formatSpeed = (bytesPerSec: number) => {
  if (bytesPerSec >= 1024 * 1024) return `${(bytesPerSec / 1024 / 1024).toFixed(1)}MB/s`
  return `${Math.max(0, Math.round(bytesPerSec / 1024))}KB/s`
}

const resetSpeedState = () => {
  displayedSpeedBytesPerSec = 0
  lastActiveSpeedBytesPerSec = 0
  lastActiveSpeedAt = 0
  currentSpeedText.value = "0KB/s"
}

const updateSmoothSpeed = ({ currentBytesPerSec, averageBytesPerSec }: DownloadSpeedPayload) => {
  const now = Date.now()
  let sample = currentBytesPerSec

  if (sample > 0) {
    lastActiveSpeedBytesPerSec = sample
    lastActiveSpeedAt = now
  } else if (lastActiveSpeedBytesPerSec > 0 && now - lastActiveSpeedAt <= SPEED_ZERO_HOLD_MS) {
    sample = lastActiveSpeedBytesPerSec
  } else {
    sample = averageBytesPerSec
  }

  if (sample <= 0) {
    displayedSpeedBytesPerSec = 0
  } else if (displayedSpeedBytesPerSec <= 0) {
    displayedSpeedBytesPerSec = sample
  } else {
    displayedSpeedBytesPerSec =
      displayedSpeedBytesPerSec * (1 - SPEED_SMOOTHING_ALPHA) + sample * SPEED_SMOOTHING_ALPHA
  }

  currentSpeedText.value = formatSpeed(displayedSpeedBytesPerSec)
}

const formatProgressPercent = (current: number, total: number) => {
  if (total <= 0) return "0%"
  return `${Math.min(100, Math.max(0, Math.round((current / total) * 100)))}%`
}

const updateDownloadStatusText = () => {
  if (!downloadProgress.value) return
  const { current, total, label } = downloadProgress.value
  statusText.value = `正在更新${label} ${formatProgressPercent(current, total)} (${currentSpeedText.value})`
}

const switchToChinaCdn = async () => {
  if (isSwitchingCdn.value) return
  isSwitchingCdn.value = true
  try {
    const newConfig = await invoke<Config>('switch_to_china_cdn')
    cache.setSettings(newConfig)
    showSwitchCdnButton.value = false
    statusText.value = "已切换至 CDN，正在重试..."
    toast.success("已切换至 CDN", { duration: 1500 })
  } catch (e) {
    toast.error("切换 CDN 失败: " + e, { duration: 10000 })
  } finally {
    isSwitchingCdn.value = false
  }
}

onMounted(async () => {
  fetchQuote();
  const hasMcDir = await invoke<boolean>('check_mc_directory');
  if (!hasMcDir) {
    statusText.value = "当前目录错误";
    toast.error("请将软件放置在.minecraft文件夹同级目录", { duration: 10000 });
    return;
  }
  unlisten = await listen<{ current: number; total: number; file: string }>(
    'download-progress',
    (event) => {
      const { current, total } = event.payload;
      if (total > 0) {
        const progressFile = event.payload.file ?? "";
        const isNamedResource = /^(Minecraft|NeoForge|Forge)\b/i.test(progressFile);
        const label = isNamedResource ? progressFile : "整合包资源";
        if (isNamedResource) {
          showSwitchCdnButton.value = false;
        }
        downloadProgress.value = { current, total, label };
        updateDownloadStatusText();
      } else {
        downloadProgress.value = null;
        resetSpeedState();
        statusText.value = event.payload.file && event.payload.file !== "/"
          ? event.payload.file
          : "检测到新版本，正在更新...";
      }
    }
  );
  unlistenSpeed = await listen<DownloadSpeedPayload>(
    'download-speed',
    (event) => {
      updateSmoothSpeed(event.payload)
      updateDownloadStatusText()
      showSwitchCdnButton.value =
        event.payload.lowSpeed
        && event.payload.source !== 'bitiful'
        && cache.settings.downloadSource !== 'bitiful'
    }
  );

  try {
    statusText.value = "正在加载核心配置...";
    const [currentUser, savedConfig, totalMemory] = await Promise.all([
      invoke<UserInfo | null>('get_current_user'),
      invoke<Config>('get_config'),
      invoke<number>('get_total_memory'),
    ]);

    if (totalMemory) cache.setTotalMem(totalMemory);
    cache.setSettings(savedConfig);
    if (currentUser) cache.setUser(currentUser);

    statusText.value = "正在校验 JAVA 可用性...";
    const checkJavaAndProceed = async () => {
      let isJavaReady = false;
      if (savedConfig.javaPath && savedConfig.javaPath.trim() !== "") {
        try {
          const validInfo = await invoke<JavaInfo>('validate_java', { path: savedConfig.javaPath });
          cache.setJavaList([validInfo]);
          isJavaReady = true;
          console.log("[Boot] Java validated:", validInfo.version);
        } catch (err) {
          console.warn("[Boot] Saved Java path is invalid, falling back to scan.");
        }
      }

      if (!isJavaReady) {
        const list = await invoke<JavaInfo[]>('scan_java_environments');
        cache.setJavaList(list);
        if (list.length > 0) {
          const firstJava = list[0];
          const newConfig = { ...savedConfig, javaPath: firstJava.path };
          cache.setSettings(newConfig);
          await invoke('save_config', { config: newConfig });
          console.log("[Boot] Java scan completed and auto-saved.");
        }
      }
    };

    await checkJavaAndProceed();

    try {
      statusText.value = "正在检查更新...";
      await invoke('sync_versions');
    } catch (e) {
      console.error("更新失败:", e);
      statusText.value = "更新失败，尝试跳过...";
      await router.replace(currentUser ? "/main" : "/login");
      toast.error("更新失败", { duration: 10000 });
      return;
    }

    statusText.value = "准备就绪...";
    setTimeout(async () => {
      await router.replace(currentUser ? "/main" : "/login");
    }, 300);
  } catch (error) {
    console.error("[Boot] Initialization failed:", error);
    await router.replace("/login");
  }
});

onUnmounted(() => {
  if (unlisten) unlisten();
  if (unlistenSpeed) unlistenSpeed();
});
</script>
