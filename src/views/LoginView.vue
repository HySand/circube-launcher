<template>
  <div class="h-full flex flex-col p-8 bg-white pt-4 animate-fade-in overflow-hidden select-none">

    <div class="flex-1 flex flex-col gap-6">

      <div class="flex-1 bg-slate-50/50 rounded-[32px] border border-slate-100 p-6 flex flex-col group transition-all duration-500 hover:bg-white hover:shadow-2xl hover:shadow-blue-500/10 hover:border-blue-100">
        <div class="flex items-center justify-between mb-4">
          <div class="flex items-center gap-2">
            <div class="w-1.5 h-1.5 bg-blue-500 rounded-full shadow-[0_0_8px_rgba(59,130,246,0.5)]"></div>
            <p class="text-[10px] font-black text-slate-800 uppercase tracking-widest">正版</p>
          </div>
          <span class="text-[8px] text-slate-300 font-bold uppercase tracking-widest leading-none">MS-OAuth2</span>
        </div>

        <div class="flex-1 flex flex-col justify-center items-center gap-4">
          <div class="w-10 h-10 bg-white rounded-xl flex items-center justify-center shadow-sm border border-slate-50 group-hover:scale-105 transition-all duration-500">
            <svg viewBox="0 0 23 23" class="w-5 h-5"><path fill="#f3f3f3" d="M0 0h23v23H0z"/><path fill="#f35325" d="M1 1h10v10H1z"/><path fill="#81bc06" d="M12 1h10v10H12z"/><path fill="#05a6f0" d="M1 12h10v10H1z"/><path fill="#ffba08" d="M12 12h10v10H12z"/></svg>
          </div>
          <div class="text-center">
            <p class="text-[10px] font-black text-slate-600 uppercase tracking-tighter">微软账号</p>
            <p class="text-[8px] text-slate-400 mt-0.5 font-medium">正版用户使用此方式</p>
          </div>
          <button @click="loginWithMicrosoft"
                  class="w-full py-3 bg-blue-600 hover:bg-blue-700 text-white rounded-xl text-[10px] font-black tracking-widest transition-all active:scale-95 shadow-lg shadow-blue-100">
            登录
          </button>
        </div>
      </div>

      <div class="flex-1 bg-slate-50/50 rounded-[32px] border border-slate-100 p-6 flex flex-col transition-all duration-500 hover:bg-white hover:shadow-2xl hover:shadow-emerald-500/10 hover:border-emerald-100">
        <div class="flex items-center justify-between mb-4">
          <div class="flex items-center gap-2">
            <div class="w-1.5 h-1.5 bg-emerald-500 rounded-full animate-pulse shadow-[0_0_8px_rgba(16,185,129,0.5)]"></div>
            <p class="text-[10px] font-black text-slate-800 uppercase tracking-widest">离线</p>
          </div>
          <span class="text-[8px] text-slate-300 font-bold uppercase tracking-widest leading-none">Yggdrasil</span>
        </div>

        <div class="space-y-3 h-full flex flex-col justify-center">
          <div class="space-y-2">
            <input v-model="authForm.email" type="text" placeholder="账号"
                   class="w-full px-4 py-3 bg-white border border-slate-100 rounded-xl text-[10px] font-medium focus:outline-none focus:border-emerald-500 focus:ring-4 focus:ring-emerald-500/5 transition-all" />
            <input v-model="authForm.password" type="password" placeholder="密码"
                   class="w-full px-4 py-3 bg-white border border-slate-100 rounded-xl text-[10px] font-medium focus:outline-none focus:border-emerald-500 focus:ring-4 focus:ring-emerald-500/5 transition-all" />
          </div>

          <div class="flex gap-2">
            <button @click="loginWithYggdrasil"
                    class="flex-1 py-3 bg-emerald-600 hover:bg-emerald-700 text-white rounded-xl text-[10px] font-black tracking-widest transition-all active:scale-95 shadow-lg shadow-emerald-100">
              登录
            </button>
            <button @click="openRegister"
                    class="px-4 py-3 bg-white border border-slate-100 text-slate-400 hover:text-slate-600 rounded-xl text-[10px] font-black transition-all">
              注册
            </button>
          </div>
        </div>
      </div>
    </div>

    <div class="mt-6 flex justify-between items-center px-2">
      <span class="text-[8px] text-slate-300 font-bold uppercase tracking-[0.2em]">MADE BY ZEPHYR</span>
      <button @click="router.push('/main')" class="text-[9px] font-black text-slate-400 hover:text-blue-500 transition-colors uppercase tracking-tight">
        Skip →
      </button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { reactive } from 'vue'
import { useRouter } from 'vue-router'
import { invoke } from '@tauri-apps/api/core'
import { openUrl } from '@tauri-apps/plugin-opener';
import {toast} from 'vue-sonner'

const router = useRouter()
const authForm = reactive({ email: '', password: '' })

// 微软正版登录
const loginWithMicrosoft = async () => {
  try {
    const res = await invoke('ms_login_command')
    console.log("微软登录状态:", res)
    await router.push('/main') // 登录成功跳转
  } catch (err) {
    alert("正版登录失败: " + err)
  }
}

// LittleSkin 登录
const loginWithYggdrasil = async () => {
  if (!authForm.email || !authForm.password) {
    toast.error("请填写账号密码")
    return
  }

  try {
    const res = await invoke('yggdrasil_login', { payload: { ...authForm } })
    console.log("LittleSkin 登录状态:", res)
    await router.push('/')
  } catch (err) {
    alert("登录错误: " + err)
  }
}

const openRegister = async () => {
  try {
    await openUrl('https://littleskin.cn/auth/register');
  } catch (err) {
    console.error("无法打开外部浏览器:", err);
  }
}
</script>