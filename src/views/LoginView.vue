<template>
  <div class="h-full flex flex-col p-10 bg-white pt-5 animate-fade-in overflow-hidden select-none">
    <div class="flex-1 flex flex-col gap-7">

      <div
        class="bg-slate-50/50 rounded-[38px] border border-slate-100 p-7 flex flex-col group transition-all duration-500 hover:bg-white hover:shadow-2xl hover:shadow-blue-500/10 hover:border-blue-100">
        <div class="flex items-center justify-between mb-5">
          <div class="flex items-center gap-2.5">
            <div class="w-2 h-2 bg-blue-500 rounded-full shadow-[0_0_10px_rgba(59,130,246,0.5)]"></div>
            <p class="text-[13px] font-black text-slate-800 uppercase tracking-widest">正版</p>
          </div>
          <span class="text-[11px] text-slate-300 font-bold uppercase tracking-widest leading-none">MS-OAuth2</span>
        </div>

        <div class="flex-1 flex flex-col justify-center items-center min-h-[156px]">
          <template v-if="!deviceCode">
            <div
              class="w-10 h-10 bg-white rounded-2xl flex items-center justify-center shadow-sm border border-slate-50 transition-all duration-500">
              <svg viewBox="0 0 23 23" class="w-5 h-5">
                <path fill="#f3f3f3" d="M0 0h23v23H0z" />
                <path fill="#f35325" d="M1 1h10v10H1z" />
                <path fill="#81bc06" d="M12 1h10v10H12z" />
                <path fill="#05a6f0" d="M1 12h10v10H1z" />
                <path fill="#ffba08" d="M12 12h10v10H12z" />
              </svg>
            </div>
            <div class="text-center mt-3.5 mb-6">
              <p class="text-[13px] text-slate-600 uppercase tracking-tighter">微软账号</p>
              <p class="text-[11px] text-slate-400 mt-1 font-medium">正版用户使用此方式</p>
            </div>
            <button @click="loginWithMicrosoft" :disabled="isMsLoading"
              class="w-full py-4 bg-blue-600 hover:bg-blue-700 text-white rounded-2xl text-[13px] font-black tracking-[0.2em] transition-all active:scale-95 shadow-lg shadow-blue-100 disabled:bg-slate-200 disabled:shadow-none">
              {{ isMsLoading ? '获取中' : '登录' }}
            </button>
          </template>

          <div v-else class="w-full flex flex-col items-center animate-in fade-in space-y-5">
            <div class="flex flex-col items-center gap-1.5">
              <p class="text-[12px] text-slate-600 font-bold uppercase">代码已复制到剪贴板</p>
            </div>

            <div @click="copyToClipboard(deviceCode)"
              class="w-full py-1.5 bg-slate-50 border-2 border-dashed border-blue-200 rounded-2xl flex items-center justify-center group/code cursor-pointer hover:border-blue-400 hover:bg-white transition-all duration-300">
              <span class="text-3xl font-black tracking-[0.3em] text-blue-600 transition-transform">
                {{ deviceCode }}
              </span>
            </div>

            <div class="flex flex-col items-center gap-2.5 pt-2.5">
              <p class="text-[9px] text-slate-400 font-medium">请在自动打开的页面中输入验证码</p>
              <button @click="deviceCode = ''; isMsLoading = false"
                class="px-5 py-1.5 text-[11px] font-black text-slate-300 hover:text-blue-500 border border-transparent hover:border-blue-100 rounded-full uppercase transition-all tracking-tighter">
                重新获取 ←
              </button>
            </div>
          </div>
        </div>
      </div>

      <div
        class="flex-1 bg-slate-50/50 rounded-[38px] border border-slate-100 p-7 flex flex-col transition-all duration-500 hover:bg-white hover:shadow-2xl hover:shadow-emerald-500/10 hover:border-emerald-100 relative overflow-hidden">
        <div class="flex items-center justify-between mb-5">
          <div class="flex items-center gap-2.5">
            <div class="w-2 h-2 bg-emerald-500 rounded-full animate-pulse shadow-[0_0_10px_rgba(16,185,129,0.5)]">
            </div>
            <p class="text-[13px] font-black text-slate-800 uppercase tracking-widest">离线</p>
          </div>
          <span class="text-[11px] text-slate-300 font-bold uppercase tracking-widest leading-none">Yggdrasil</span>
        </div>

        <div class="flex-1 relative min-h-[156px]">
          <div v-if="!showProfileSelector"
            class="absolute inset-0 flex flex-col justify-center space-y-3 animate-in fade-in duration-300">
            <div class="space-y-2">
              <input v-model="authForm.email" type="text" placeholder="邮箱或用户名"
                class="w-full px-5 py-3 bg-white border border-slate-100 rounded-2xl text-[12px] font-medium focus:outline-none focus:border-emerald-500 transition-all" />
              <input v-model="authForm.password" type="password" placeholder="密码"
                class="w-full px-5 py-3 bg-white border border-slate-100 rounded-2xl text-[12px] font-medium focus:outline-none focus:border-emerald-500 transition-all" />
            </div>
            <div class="flex gap-2.5">
              <button @click="loginWithYggdrasil" :disabled="isLoading"
                class="flex-1 py-3 bg-emerald-600 hover:bg-emerald-700 text-white rounded-2xl text-[12px] font-black tracking-widest transition-all active:scale-95 shadow-lg shadow-emerald-100">
                {{ isLoading ? '验证中' : '登录' }}
              </button>
              <button @click="openRegister"
                class="px-4 py-3 bg-white border border-slate-100 text-slate-400 hover:text-slate-600 rounded-2xl text-[12px] font-black transition-all">
                注册
              </button>
            </div>
          </div>

          <div v-else class="absolute inset-0 flex flex-col space-y-2.5 animate-in slide-in-from-right-4 duration-500">
            <p class="text-[10px] font-bold text-slate-400 uppercase tracking-tight text-center shrink-0">选择角色</p>
            <div class="overflow-y-auto pr-1.5 space-y-1.5 custom-scrollbar">
              <button v-for="profile in availableProfiles" :key="profile.id" @click="selectProfile(profile)"
                class="w-full h-[53px] px-4 bg-white border border-slate-100 hover:border-emerald-500 rounded-2xl flex items-center gap-2.5 transition-all group/profile shrink-0">
                <div class="w-7 h-7 bg-slate-50 rounded-md flex-shrink-0 flex items-center justify-center">
                  <img :src="`https://littleskin.cn/avatar/player/${profile.name}?size=48`"
                    class="w-full h-full object-contain" style="image-rendering: pixelated;" />
                </div>
                <span class="text-[11px] font-bold text-slate-600 truncate">{{ profile.name }}</span>
              </button>
            </div>
            <button @click="showProfileSelector = false"
              class="w-full py-1.5 text-[10px] text-slate-300 hover:text-slate-500 uppercase transition-colors shrink-0">
              返回
            </button>
          </div>
        </div>
      </div>
    </div>

    <div class="mt-5 flex justify-between items-center px-2.5 shrink-0">
      <span class="text-[10px] text-slate-200 font-bold uppercase tracking-[0.2em]">MADE BY ZEPHYR</span>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, reactive, onMounted, onUnmounted } from 'vue'
import { useRouter } from 'vue-router'
import { invoke } from '@tauri-apps/api/core'
import { listen, UnlistenFn } from '@tauri-apps/api/event'
import { openUrl } from '@tauri-apps/plugin-opener'
import { toast } from 'vue-sonner'

type AuthResponse =
  | { status: 'success'; data: any }
  | { status: 'need_selection'; data: { profiles: any[]; access_token: string; client_token: string } };

const router = useRouter()
const authForm = reactive({ email: '', password: '' })
const isLoading = ref(false)

const isMsLoading = ref(false)
const msLoadingText = ref("登录")
const deviceCode = ref("")

const showProfileSelector = ref(false)
const availableProfiles = ref<any[]>([])
const selectionContext = ref<{ accessToken: string, clientToken: string } | null>(null)

let unlistenStatus: UnlistenFn | null = null
let unlistenSuccess: UnlistenFn | null = null
let unlistenError: UnlistenFn | null = null

onMounted(async () => {
  // 统一事件监听逻辑
  unlistenStatus = await listen<any>('ms-status', (event) => {
    const msg = typeof event.payload === 'string' ? event.payload : event.payload.message
    msLoadingText.value = msg
    if (msg.includes("错误")) {
      toast.error(msg, { duration: 10000 })
      isMsLoading.value = false
      deviceCode.value = ""
    }
  })

  unlistenSuccess = await listen<any>('ms-login-success', () => {
    isMsLoading.value = false
    deviceCode.value = ""
    toast.success("微软登录成功！", { duration: 1500 })
    router.push('/')
  })

  unlistenError = await listen<any>('ms-login-error', (event) => {
    isMsLoading.value = false
    deviceCode.value = ""
    const msg = typeof event.payload === 'string' ? event.payload : '微软登录失败'
    toast.error(msg, { duration: 10000 })
  })
})

onUnmounted(() => {
  if (unlistenStatus) unlistenStatus()
  if (unlistenSuccess) unlistenSuccess()
  if (unlistenError) unlistenError()
})

const copyToClipboard = async (text: string) => {
  try {
    await navigator.clipboard.writeText(text);
    toast.success("代码已重新复制", { duration: 1500 });
  } catch (err) {
    console.error("Clipboard error: ", err);
  }
};

const loginWithMicrosoft = async () => {
  if (isMsLoading.value) return;

  isMsLoading.value = true;
  msLoadingText.value = "正在请求微软服务器...";

  try {
    const code = await invoke<string>('ms_login');
    deviceCode.value = code;
    await copyToClipboard(code);
  }
  catch (err) {
    isMsLoading.value = false;
    toast.error("初始化失败: " + err, { duration: 10000 });
  }
};

const loginWithYggdrasil = async () => {
  if (!authForm.email || !authForm.password) { toast.error("请输入账号密码", { duration: 10000 }); return; }
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
    toast.error("登录失败: " + err, { duration: 10000 })
  } finally { isLoading.value = false; }
}

const selectProfile = async (profile: any) => {
  if (!selectionContext.value) return
  isLoading.value = true
  try {
    await invoke('yggdrasil_select', {
      payload: {
        accessToken: selectionContext.value.accessToken,
        clientToken: selectionContext.value.clientToken,
        profile: { id: profile.id, name: profile.name }
      }
    })
    await router.push('/')
  } catch (err) { toast.error("角色选择失败: " + err, { duration: 10000 }); }
  finally { isLoading.value = false; }
}

const openRegister = async () => { await openUrl('https://littleskin.cn/auth/register'); }
</script>
