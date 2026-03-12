<template>
  <div class="w-screen h-screen overflow-hidden rounded-2xl border relative flex flex-col border-border">
    <TitleBar/>
    <Toaster richColors position="top-center" :duration="1500"/>
    <div class="flex-1 pt-10 overflow-y-auto" >
      <router-view />
    </div>
  </div>
</template>

<script setup lang="ts">
import TitleBar from './components/TitleBar.vue';
import { Toaster } from '@/components/ui/sonner'
import 'vue-sonner/style.css'
</script>

<style>
[data-sonner-toast] {
  backdrop-filter: blur(12px) !important;
  -webkit-backdrop-filter: blur(12px) !important;

  /* 使用 LoginView 的背景色调 */
  background-color: rgba(255, 255, 255, 0.8) !important;
  border: 1px solid rgba(241, 245, 249, 1) !important; /* slate-100 */
  border-radius: 16px !important;
  /* 调整投影，更具悬浮感 */
  box-shadow: 0 10px 25px -5px rgba(0, 0, 0, 0.05), 0 8px 10px -6px rgba(0, 0, 0, 0.05) !important;
}

/* 3. 修复 richColors 开启时的遮盖问题 */
[data-sonner-toast][data-rich-colors="true"] {
  background-clip: padding-box !important;
}

/* 成功状态：对齐 LoginView 的 Emerald 风格 */
[data-sonner-toast][data-type="success"] {
  background-color: rgba(240, 253, 244, 0.2) !important; /* emerald-50 */
  border-color: rgba(187, 247, 208, 0.1) !important;   /* emerald-200 */
  color: #10b981 !important;
}

/* 错误状态：对齐 LoginView 潜在的告警风格 */
[data-sonner-toast][data-type="error"] {
  background-color: rgba(254, 242, 242, 0.2) !important; /* red-50 */
  border-color: rgba(254, 226, 226, 0.1) !important;    /* red-200 */
  color: #ef4444 !important;
}

/* 修复 Toast 内部图标和文字的精细度 */
[data-sonner-toast] [data-title] {
  font-weight: 800 !important;
  font-size: 11px !important;
  text-transform: uppercase !important;
  letter-spacing: 0.05em !important;
}
</style>