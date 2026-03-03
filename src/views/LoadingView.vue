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
  fetchQuote()
  try {
    const [currentUser, savedConfig, totalMemory, javaList] = await Promise.all([
      invoke<UserInfo | null>('get_current_user'),
      invoke<Config>('get_config'),
      invoke<number>('get_total_memory'),
      invoke<JavaInfo[]>('scan_java_environments'),
    ]);

    // 1. 基础数据注入 Store
    if (totalMemory) cache.setTotalMem(totalMemory);
    if (javaList) cache.setJavaList(javaList);

    // 2. Java 路径决策逻辑
    let finalConfig = { ...savedConfig };
    let needSilenceSave = false;

    if (javaList && javaList.length > 0) {
      // 检查当前路径是否有效
      const isPathValid = javaList.some(j => j.path === savedConfig.javaPath);

      // 仅在路径为空或无效时进行选择
      if (!savedConfig.javaPath || savedConfig.javaPath === "" || !isPathValid) {
        finalConfig.javaPath = javaList[0].path;
        needSilenceSave = true;
        console.log("[Boot] Auto-selected valid Java:", finalConfig.javaPath);
      }
    }

    // 3. 更新配置 Store
    cache.setSettings(finalConfig);

    // 4. 静默持久化
    if (needSilenceSave) {
      await invoke('save_config', { config: finalConfig });
    }

    // 5. 路由分发
    if (currentUser) {
      cache.setUser(currentUser);
      await router.replace("/main");
    } else {
      await router.replace("/login");
    }
  } catch (error) {
    console.error("[Boot] Initialization failed:", error);
    await router.replace("/login");
  }
})
</script>