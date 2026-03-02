<template>
  <div data-tauri-drag-region class="h-10 w-full flex justify-between items-center px-4 bg-white/80 backdrop-blur-md absolute top-0 z-50">
    <div class="flex items-center gap-2 pointer-events-none">
      <img src="@/assets/circube.svg" class="w-3 h-3 translate-y-[-0.5px]" alt="Logo" />
      <span class="text-[10px] font-black text-slate-400 uppercase tracking-widest">CirCube Launcher</span>
    </div>

    <div class="flex gap-4">
      <button @click="minimize" class="text-slate-400 hover:text-slate-800 transition-colors">
        <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M5 12h14"/></svg>
      </button>
      <button @click="close" class="text-slate-400 hover:text-red-500 transition-colors">
        <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M18 6L6 18M6 6l12 12"/></svg>
      </button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { getCurrentWindow } from '@tauri-apps/api/window';

const appWindow = getCurrentWindow();

const minimize = async () => {
  await appWindow.minimize();
};

const close = async () => {
  await appWindow.close();
};
</script>