<template>
  <div class="h-full flex flex-col bg-white px-8 pt-4 pb-6 overflow-hidden select-none">
    <header class="mb-4 flex items-start justify-between">
      <h2 class="text-2xl font-black text-slate-800 tracking-tighter leading-none">
        你好, <span class="text-blue-600">{{ username }}</span>
      </h2>

      <button @click="handleSettings"
              class="p-2.5 bg-slate-50 hover:bg-slate-100 text-slate-400 hover:text-blue-600 rounded-xl transition-all duration-300 active:scale-90 group border border-slate-100/50">
        <svg xmlns="http://www.w3.org/2000/svg" class="w-5 h-5 group-hover:rotate-90 transition-transform duration-500" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
          <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/>
        </svg>
      </button>
    </header>

    <div class="mb-4 min-h-[48px] flex flex-col justify-center">
      <p v-if="quote.text" class="text-slate-400 text-[11px] leading-relaxed italic font-medium line-clamp-2">
        " {{ quote.text }} "
      </p>
      <p v-if="quote.from" class="text-[9px] text-slate-300 mt-1 font-bold uppercase tracking-wider text-right w-full">
        — {{ quote.from }}
      </p>
    </div>

    <div class="flex-1 flex justify-center items-center min-h-0">
      <div class="h-full aspect-[1/2] bg-slate-50/50 rounded-[48px] border border-slate-100 flex items-center justify-center relative overflow-hidden group shadow-inner transition-all duration-500 hover:bg-white hover:border-blue-100">
        <div class="absolute inset-0 bg-gradient-to-br from-blue-50/20 via-transparent to-slate-100/30"></div>
        <div ref="container" class="z-10 pt-4 flex flex-col items-center"></div>
      </div>
    </div>

    <div class="mt-8 flex flex-col items-center gap-2">
      <p v-if="isLaunching" class="text-[10px] text-blue-500 font-bold animate-pulse tracking-widest uppercase">
        {{ launchStatus }}
      </p>
      <button @click="handleLaunch"
              :disabled="isLaunching"
              class="w-full py-5 text-white rounded-[24px] text-[12px] font-black tracking-widest shadow-lg transition-all duration-300 active:scale-95 flex items-center justify-center gap-3 group"
              :class="[isLaunching ? 'bg-slate-300 cursor-not-allowed' : 'bg-blue-600 hover:bg-blue-700 shadow-blue-100']">
        <template v-if="!isLaunching">
          <span>CirCube 启动！</span>
          <svg xmlns="http://www.w3.org/2000/svg" class="w-5 h-5 group-hover:translate-y-[-2px] transition-transform" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round">
            <path d="m5 12 7-7 7 7"/><path d="M12 19V5"/>
          </svg>
        </template>
        <template v-else>
          <svg class="animate-spin h-5 w-5 text-white" fill="none" viewBox="0 0 24 24">
            <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
            <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path>
          </svg>
          <span>CirCube运行中</span>
        </template>
      </button>
    </div>
  </div>
</template>

<script setup lang="ts">
import {ref, onMounted, onUnmounted, watch, nextTick} from "vue";
import { useRouter } from "vue-router";
import { useCacheStore } from '@/stores/cache';
import * as skinview3d from 'skinview3d';
import {invoke} from "@tauri-apps/api/core";
import {listen, UnlistenFn} from "@tauri-apps/api/event";

const router = useRouter();
const cache = useCacheStore();

const username = ref(cache.user?.name ?? "");
const quote = ref(cache.quote);
const skinUrl = ref(cache.user?.skinUrl ?? "https://textures.minecraft.net/texture/58806cf80200b93b3a3471ef0fc0326a4bb8daf69af7d2929764e4149988882e");

const container = ref<HTMLElement | null>(null);
let viewer: skinview3d.SkinViewer | null = null;

// 新增：启动状态变量
const isLaunching = ref(false);
const launchStatus = ref("");
let unlistenStatus: UnlistenFn;
let unlistenExit: UnlistenFn;

onMounted(async () => {
  // 保持原有皮肤预览逻辑
  if (container.value && skinUrl.value) {
    viewer = new skinview3d.SkinViewer({
      canvas: document.createElement('canvas'),
      width: 260,
      height: 320,
      skin: skinUrl.value
    });
    container.value.appendChild(viewer.canvas);
    viewer.animation = new skinview3d.IdleAnimation();
  }

  // 监听进度状态
  unlistenStatus = await listen<string>("launch-status", (event) => {
    launchStatus.value = event.payload;
  });

  // 监听退出
  unlistenExit = await listen("game-exited", () => {
    isLaunching.value = false;
    launchStatus.value = "";
  });
});

onUnmounted(() => {
  if (unlistenStatus) unlistenStatus();
  if (unlistenExit) unlistenExit();
});

watch(() => cache.quote, async (newQuote) => {
  quote.value = newQuote;
  await nextTick();
});

const handleLaunch = async () => {
  if (isLaunching.value) return;
  isLaunching.value = true;
  launchStatus.value = "正在初始化...";
  try {
    await invoke("launch_minecraft");
  } catch (e) {
    isLaunching.value = false;
    console.error(e);
  }
};

const handleSettings = () => {
  router.push("/settings");
};
</script>