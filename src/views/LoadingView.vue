<template>
  <div class="h-full flex items-center justify-center bg-white animate-fade-in">
      <Spinner class="text-blue-700"/>
  </div>
</template>

<script setup lang="ts">
import { onMounted } from "vue"
import { useRouter } from "vue-router"
import { useCacheStore } from '@/stores/cache'
import { invoke } from "@tauri-apps/api/core";
import { Spinner } from "@/components/ui/spinner"

const router = useRouter()
const cache = useCacheStore()

const fetchQuote = async () => {
  try {
    const response = await fetch("https://v1.hitokoto.cn/?c=c")
    const data = await response.json()
    cache.setQuote({ text: data.hitokoto, from: data.from })
  } catch {
    cache.setQuote({ text: "Stay hungry, stay foolish.", from: "Steve Jobs" })
  }
}

interface UserInfo { uuid: string; name: string; accessToken: string; skinUrl: string; authType: string }
interface Config { javaPath: string; maxMemory: number }
interface JavaInfo { path: string; version: string }

onMounted(async () => {
  fetchQuote();
  try {
    // 第一步：只拿核心配置和用户信息（速度极快）
    const [currentUser, savedConfig, totalMemory] = await Promise.all([
      invoke<UserInfo | null>('get_current_user'),
      invoke<Config>('get_config'),
      invoke<number>('get_total_memory'),
    ]);

    if (totalMemory) cache.setTotalMem(totalMemory);
    cache.setSettings(savedConfig);
    if (currentUser) cache.setUser(currentUser);

    // 第二步：判断是否需要执行 Java 扫描
    const checkJavaAndProceed = async () => {
      let isJavaReady = false;

      // 场景 A: 存在保存的路径 -> 执行轻量级验证
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

      // 场景 B: 路径不存在或验证失败 -> 执行全量扫描
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

    // 执行 Java 检查逻辑（不 await，避免阻塞跳转）
    await checkJavaAndProceed();

    await router.replace(currentUser ? "/main" : "/login");
  } catch (error) {
    console.error("[Boot] Initialization failed:", error);
    await router.replace("/login");
  }
});
</script>