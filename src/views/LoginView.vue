<template>
  <div class="h-full flex flex-col p-8 bg-white pt-4 animate-fade-in overflow-hidden select-none">
    <div class="flex-1 flex flex-col gap-6">

      <div class="bg-slate-50/50 rounded-[32px] border border-slate-100 p-6 flex flex-col group transition-all duration-500 hover:bg-white hover:shadow-2xl hover:shadow-blue-500/10 hover:border-blue-100">
        <div class="flex items-center justify-between mb-4">
          <div class="flex items-center gap-2">
            <div class="w-1.5 h-1.5 bg-blue-500 rounded-full shadow-[0_0_8px_rgba(59,130,246,0.5)]"></div>
            <p class="text-[10px] font-black text-slate-800 uppercase tracking-widest">正版</p>
          </div>
          <span class="text-[8px] text-slate-300 font-bold uppercase tracking-widest leading-none">MS-OAuth2</span>
        </div>

        <div class="flex-1 flex flex-col justify-center items-center gap-3 min-h-[130px]">
          <div class="w-9 h-9 bg-white rounded-xl flex items-center justify-center shadow-sm border border-slate-50 group-hover:scale-105 transition-all duration-500">
            <svg viewBox="0 0 23 23" class="w-4 h-4"><path fill="#f3f3f3" d="M0 0h23v23H0z"/><path fill="#f35325" d="M1 1h10v10H1z"/><path fill="#81bc06" d="M12 1h10v10H12z"/><path fill="#05a6f0" d="M1 12h10v10H1z"/><path fill="#ffba08" d="M12 12h10v10H12z"/></svg>
          </div>
          <div class="text-center">
            <p class="text-[9px] font-black text-slate-600 uppercase tracking-tighter">微软账号</p>
            <p class="text-[8px] text-slate-400 mt-0.5 font-medium">正版用户使用此方式</p>
          </div>
          <button @click="loginWithMicrosoft"
                  class="w-full py-2.5 bg-blue-600 hover:bg-blue-700 text-white rounded-xl text-[10px] font-black tracking-widest transition-all active:scale-95 shadow-lg shadow-blue-100">
            登录
          </button>
        </div>
      </div>

      <div class="flex-1 bg-slate-50/50 rounded-[32px] border border-slate-100 p-6 flex flex-col transition-all duration-500 hover:bg-white hover:shadow-2xl hover:shadow-emerald-500/10 hover:border-emerald-100 relative overflow-hidden">
        <div class="flex items-center justify-between mb-4">
          <div class="flex items-center gap-2">
            <div class="w-1.5 h-1.5 bg-emerald-500 rounded-full animate-pulse shadow-[0_0_8px_rgba(16,185,129,0.5)]"></div>
            <p class="text-[10px] font-black text-slate-800 uppercase tracking-widest">离线</p>
          </div>
          <span class="text-[8px] text-slate-300 font-bold uppercase tracking-widest leading-none">Yggdrasil</span>
        </div>

        <div class="flex-1 relative min-h-[130px]">
          <div v-if="!showProfileSelector" class="absolute inset-0 flex flex-col justify-center space-y-2.5 animate-in fade-in duration-300">
            <div class="space-y-1.5">
              <input v-model="authForm.email" type="text" placeholder="账号"
                     class="w-full px-4 py-2.5 bg-white border border-slate-100 rounded-xl text-[10px] font-medium focus:outline-none focus:border-emerald-500 transition-all" />
              <input v-model="authForm.password" type="password" placeholder="密码"
                     class="w-full px-4 py-2.5 bg-white border border-slate-100 rounded-xl text-[10px] font-medium focus:outline-none focus:border-emerald-500 transition-all" />
            </div>

            <div class="flex gap-2">
              <button @click="loginWithYggdrasil" :disabled="isLoading"
                      class="flex-1 py-2.5 bg-emerald-600 hover:bg-emerald-700 text-white rounded-xl text-[10px] font-black tracking-widest transition-all active:scale-95 shadow-lg shadow-emerald-100">
                {{ isLoading ? '验证中' : '登录' }}
              </button>
              <button @click="openRegister"
                      class="px-3 py-2.5 bg-white border border-slate-100 text-slate-400 hover:text-slate-600 rounded-xl text-[10px] font-black transition-all">
                注册
              </button>
            </div>
          </div>

          <div v-else class="absolute inset-0 flex flex-col space-y-2 animate-in slide-in-from-right-4 duration-500">
            <p class="text-[8px] font-bold text-slate-400 uppercase tracking-tight text-center shrink-0">选择角色</p>

            <div class="overflow-y-auto pr-1 space-y-1 custom-scrollbar">
              <button v-for="profile in availableProfiles" :key="profile.id"
                      @click="selectProfile(profile)"
                      class="w-full h-[44px] px-3 bg-white border border-slate-100 hover:border-emerald-500 rounded-xl flex items-center gap-2 transition-all group/profile shrink-0">
                <div class="w-6 h-6 bg-slate-50 rounded flex-shrink-0 flex items-center justify-center">
                  <img :src="`https://littleskin.cn/avatar/player/${profile.name}?size=48`"
                       class="w-full h-full object-contain"
                       style="image-rendering: pixelated;" />
                </div>
                <span class="text-[9px] font-bold text-slate-600 truncate">{{ profile.name }}</span>
              </button>
            </div>

            <button @click="showProfileSelector = false" class="w-full py-1 text-[8px] font-black text-slate-300 hover:text-slate-500 uppercase transition-colors shrink-0">
              返回
            </button>
          </div>
        </div>
      </div>
    </div>

    <div class="mt-4 flex justify-between items-center px-2 shrink-0">
      <span class="text-[8px] text-slate-200 font-bold uppercase tracking-[0.2em]">MADE BY ZEPHYR</span>
      <button @click="router.push('/main')" class="text-[8px] font-black text-slate-300 hover:text-blue-500 transition-colors uppercase">
        Skip →
      </button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, reactive } from 'vue'
import { useRouter } from 'vue-router'
import { invoke } from '@tauri-apps/api/core'
import { openUrl } from '@tauri-apps/plugin-opener'
import { toast } from 'vue-sonner'

// 定义后端枚举响应结构
type AuthResponse =
    | { status: 'success'; data: any }
    | { status: 'need_selection'; data: { profiles: any[]; access_token: string; client_token: string } };

const router = useRouter()
const authForm = reactive({ email: '', password: '' })
const isLoading = ref(false)

const showProfileSelector = ref(false)
const availableProfiles = ref<any[]>([])
const selectionContext = ref<{ accessToken: string, clientToken: string } | null>(null)

const loginWithMicrosoft = async () => {
  try { await invoke('ms_login_command'); await router.push('/main'); }
  catch (err) { toast.error("微软登录失败: " + err); }
}

const loginWithYggdrasil = async () => {
  if (!authForm.email || !authForm.password) { toast.error("请输入账号密码"); return; }
  isLoading.value = true
  try {
    const res = await invoke<AuthResponse>('yggdrasil_login', { payload: { ...authForm } })
    if (res.status === 'success') { await router.push('/'); }
    else if (res.status === 'need_selection') {
      availableProfiles.value = res.data.profiles
      selectionContext.value = { accessToken: res.data.access_token, clientToken: res.data.client_token }
      showProfileSelector.value = true
    }
  } catch (err: any) {
    const msg = String(err)

    if (msg.includes("Incorrect username or password")) {
      toast.error("账号或密码错误")
    } else {
      toast.error("登录失败")
    }
  } finally { isLoading.value = false; }
}

const selectProfile = async (profile: any) => {
  if (!selectionContext.value) return
  isLoading.value = true
  try {
    await invoke('yggdrasil_select', {
      access_token: selectionContext.value.accessToken,
      client_token: selectionContext.value.clientToken,
      profile: { id: profile.id, name: profile.name }
    })
    await router.push('/')
  } catch (err) { toast.error("选角失败: " + err); }
  finally { isLoading.value = false; }
}

const openRegister = async () => { await openUrl('https://littleskin.cn/auth/register'); }
</script>

<style scoped>
/* 强制像素渲染解决模糊 */
img {
  image-rendering: -webkit-optimize-contrast;
  image-rendering: pixelated;
}

/* 局部滚动条：极致简约设计 */
.custom-scrollbar::-webkit-scrollbar {
  width: 3px;
}
.custom-scrollbar::-webkit-scrollbar-track {
  background: transparent;
}
.custom-scrollbar::-webkit-scrollbar-thumb {
  background: #e2e8f0;
  border-radius: 10px;
}
.custom-scrollbar::-webkit-scrollbar-thumb:hover {
  background: #10b981; /* 悬浮时变 emerald 色 */
}

/* 基础进场动画 */
.animate-in {
  animation: slideIn 0.4s cubic-bezier(0.16, 1, 0.3, 1);
}
@keyframes slideIn {
  from { opacity: 0; transform: translateX(10px); }
  to { opacity: 1; transform: translateX(0); }
}
</style>