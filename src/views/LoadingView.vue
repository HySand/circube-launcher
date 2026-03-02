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

interface UserInfo{
  uuid: string
  name: string
  accessToken: string
  skinUrl: string
  authType: string
}

interface Config{
  javaPath: string
  maxMemory: number
}
interface JavaInfo { path: string; version: string }

onMounted(async () => {
  fetchQuote()
  const [currentUser, savedConfig, totalMemory, JavaList] = await Promise.all([
    invoke<UserInfo>('get_current_user'),
    invoke<Config>('get_config'),
    invoke<number>('get_total_memory'),
    invoke<JavaInfo[]>('scan_java_environments'),
  ]);
  if (currentUser) {
    cache.setUser(currentUser);
  }
  if (savedConfig) {
    cache.setSettings(savedConfig);
  }
  if (totalMemory) {
    cache.setTotalMem(totalMemory);
  }
  if (JavaList) {
    cache.setJavaList(JavaList);
  }
  if (currentUser) {
    cache.setUser(currentUser);
    await router.replace("/main")
  } else {
    await router.replace("/login")
  }
})
</script>