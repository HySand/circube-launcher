<template>
  <div class="h-full flex flex-col items-center justify-center bg-white animate-fade-in gap-4">
    <Spinner class="text-blue-700" />

    <p class="text-sm text-gray-500 font-medium tracking-tight animate-pulse min-h-[1.25rem]">
      {{ statusText }}
    </p>
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
let unlisten: UnlistenFn;

const fetchQuote = async () => {
  const response = await fetch("https://v1.hitokoto.cn/?c=d")
  const data = await response.json()
  cache.setQuote({ text: data.hitokoto, from: data.from })
}

interface UserInfo { uuid: string; name: string; accessToken: string; skinUrl: string; authType: string }
interface Config { javaPath: string; maxMemory: number }
interface JavaInfo { path: string; version: string }

onMounted(async () => {
  fetchQuote();
  unlisten = await listen<{ current: number; total: number; file: string }>(
    'download-progress',
    (event) => {
      const { current, total } = event.payload;
      statusText.value = `正在更新资源文件 (${current}/${total})`;
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
        invoke<JavaInfo[]>('scan_java_environments').then(async (list) => {
          if (list && list.length > 0) {
            cache.setJavaList(list);
            const firstJava = list[0];
            const newConfig = { ...savedConfig, javaPath: firstJava.path };
            cache.setSettings(newConfig);
            await invoke('save_config', { config: newConfig });
            console.log("[Boot] Background scan completed and auto-saved.");
          }
        });
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
      toast.error("更新失败", { duration: 1500 });
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
});
</script>