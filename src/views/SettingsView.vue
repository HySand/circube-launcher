<template>
  <div class="h-full flex flex-col bg-white overflow-hidden select-none">
    <header class="p-7 flex items-center border-b border-slate-100 flex-none">
      <Button variant="ghost" size="icon" @click="router.back()"
              class="mr-2.5 rounded-full text-slate-400 hover:bg-slate-50 transition-transform active:scale-90">
        <ArrowLeftIcon :size="24" />
      </Button>
      <h2 class="text-[22px] font-black text-slate-800 uppercase tracking-tighter">设置</h2>
    </header>

    <div class="flex-1 overflow-y-auto p-7 space-y-12 custom-scroll">
      <section class="space-y-5">
        <div class="flex items-center justify-between px-1">
          <h3 class="text-[13px] font-black text-slate-400 uppercase tracking-[0.2em]">JDK选择</h3>
          <button @click="handleManualImport"
                  class="text-[12px] font-black text-blue-600 hover:text-blue-700 uppercase transition-colors flex items-center gap-1.5 active:opacity-70">
            手动导入
          </button>
        </div>

        <Popover v-model:open="open">
          <PopoverTrigger as-child>
            <Button ref="triggerRef" variant="outline" role="combobox"
                    class="w-full justify-between h-auto py-5 px-6 border-slate-100 rounded-[19px] bg-slate-50/50 hover:bg-white hover:border-blue-100 transition-all shadow-sm group">
              <div class="flex flex-col items-start gap-2 overflow-hidden text-left">
                <span class="text-[13px] font-black tracking-widest text-blue-600">
                  {{ selectedJava?.version || (javaDetailList.length > 0 ? '请选择 Java 环境' : '未检测到 Java') }}
                </span>
                <span class="text-[11px] text-slate-400 truncate font-mono opacity-60 w-full">
                  {{ selectedJava?.path || '等待扫描...' }}
                </span>
              </div>
              <ChevronsUpDown class="ml-2.5 h-5 w-5 shrink-0 opacity-30 text-slate-400 group-hover:opacity-60 transition-opacity" />
            </Button>
          </PopoverTrigger>
          <PopoverContent class="p-1.5 border border-slate-10 rounded-[19px] shadow-2xl bg-white/20 backdrop-blur-md"
                          :style="{ width: `${triggerWidth}px`, minWidth: `${triggerWidth}px` }" align="start" :side-offset="10">
            <Command class="bg-transparent">
              <CommandList class="max-h-[336px] overflow-y-auto custom-scroll">
                <CommandEmpty class="py-12 text-[12px] text-center font-bold text-slate-300 tracking-widest">
                  系统内未发现可用的 Java 环境
                </CommandEmpty>
                <CommandGroup>
                  <CommandItem v-for="java in javaDetailList" :key="java.path" :value="java.path"
                               @select="handleJavaSelect(java.path)"
                               class="flex items-center justify-between p-4 mb-1.5 rounded-2xl cursor-pointer hover:bg-slate-50/30 group">
                    <div class="flex flex-col gap-1.5 overflow-hidden">
                      <span
                          class="font-black text-[13px] tracking-tight text-slate-700 group-hover:text-blue-600 transition-colors">
                        {{ java.version }}
                      </span>
                      <span class="text-[11px] text-slate-400 truncate font-mono opacity-50">{{ java.path }}</span>
                    </div>
                    <Check
                        :class="cn('ml-auto h-5 w-5 text-blue-600 transition-all', config.javaPath === java.path ? 'opacity-100 scale-100' : 'opacity-0 scale-50')" />
                  </CommandItem>
                </CommandGroup>
              </CommandList>
            </Command>
          </PopoverContent>
        </Popover>
      </section>

      <section class="space-y-7">
        <div class="flex items-center justify-between px-1">
          <h3 class="text-[13px] font-black text-slate-400 uppercase tracking-[0.2em]">内存分配</h3>
          <div class="flex gap-5">
            <button @click="autoTuneMemory" :disabled="isTuning"
                    class="text-[12px] font-black text-blue-600 hover:text-blue-700 uppercase disabled:opacity-30">
              {{ isTuning ? '探测中' : '自动优化' }}
            </button>
            <button @click="handleManualToggle" class="text-[12px] font-black uppercase transition-colors"
                    :class="isManualMem ? 'text-emerald-600' : 'text-slate-300 hover:text-slate-400'">
              {{ isManualMem ? '保存' : '手动' }}
            </button>
          </div>
        </div>

        <div class="relative pt-2.5 px-1">
          <div class="relative h-2.5 w-full bg-slate-100 rounded-full overflow-hidden shadow-inner">
            <div class="absolute left-0 top-0 h-full bg-blue-700 transition-all duration-1000"
                 :style="{ width: `${otherUsedPercent}%` }" />
            <div
                class="absolute top-0 h-full bg-blue-500 shadow-[0_0_12px_rgba(59,130,246,0.4)] transition-all duration-500"
                :style="{ left: `${otherUsedPercent}%`, width: `${gamePercent}%` }" />
          </div>
        </div>

        <div class="grid grid-cols-3 gap-2.5 px-1">
          <div class="flex flex-col gap-2.5">
            <Badge variant="secondary"
                   class="w-fit bg-slate-100 text-blue-700 hover:bg-slate-100 px-2 py-0.5 text-[11px] tracking-[0.1em] rounded-lg shadow-none border-none">
              系统内存</Badge>
            <span class="text-blue-800 font-numeric font-bold text-[13px] ml-1">
              {{ (totalSystemMem / 1024).toFixed(1) }} <small class="opacity-50">GB</small>
            </span>
          </div>

          <div class="flex flex-col items-center gap-2.5">
            <Badge
                class="bg-blue-50 text-blue-600 hover:bg-blue-50 px-2 py-0.5 text-[11px] tracking-[0.1em] rounded-lg shadow-none border-none">
              游戏分配</Badge>
            <span class="text-blue-700 font-numeric font-bold text-[13px]">{{ config.maxMemory }} <small
                class="opacity-50">MB</small></span>
          </div>

          <div class="flex flex-col items-end gap-2.5">
            <Badge variant="secondary"
                   class="bg-green-50/50 text-green-600 border-green-100 px-2 py-0.5 text-[11px] tracking-[0.1em] rounded-lg shadow-none">
              剩余内存</Badge>
            <span class="text-slate-600 font-numeric font-bold text-[13px] mr-1">
              {{ (freeMemAfterAlloc / 1024).toFixed(1) }} <small class="opacity-50">GB</small>
            </span>
          </div>
        </div>

        <transition enter-active-class="transition duration-300 ease-out" enter-from-class="opacity-0 -translate-y-2">
          <div v-if="isManualMem" class="px-1 py-2.5">
            <Slider v-model="sliderValue" :min="512" :max="maxSafeMemory" :step="512"
                    class="[&_[role=slider]]:bg-blue-600 [&_[role=slider]]:border-blue-600 [&_.relative_span:first-child]:bg-blue-600/20 [&_[data-orientation=horizontal]\_span:first-child]:bg-blue-600"
                    @update:model-value="handleSliderChange" />
          </div>
        </transition>
      </section>

      <section class="space-y-5">
        <div class="flex items-center justify-between px-1">
          <h3 class="text-[13px] font-black text-slate-400 uppercase tracking-[0.2em]">下载源</h3>
        </div>

        <div class="grid grid-cols-2 gap-4">
          <button type="button" @click="handleDownloadSourceSelect('r2')"
                  :class="cn('min-w-0 rounded-[19px] border p-5 text-left transition-all active:scale-[0.98]', config.downloadSource === 'r2' ? 'border-blue-200 bg-blue-50/70 text-blue-700 shadow-sm' : 'border-slate-100 bg-slate-50/50 text-slate-500 hover:bg-white hover:border-blue-100')">
            <div class="flex min-w-0 items-center gap-2.5">
              <Globe2 :size="19" class="shrink-0" />
              <span class="truncate text-[13px] font-black">源站</span>
            </div>
            <p class="mt-2.5 truncate text-[11px] font-mono opacity-60">推荐，部分地区无法使用</p>
          </button>

          <button type="button" @click="handleDownloadSourceSelect('bitiful')"
                  :class="cn('min-w-0 rounded-[19px] border p-5 text-left transition-all active:scale-[0.98]', config.downloadSource === 'bitiful' ? 'border-blue-200 bg-blue-50/70 text-blue-700 shadow-sm' : 'border-slate-100 bg-slate-50/50 text-slate-500 hover:bg-white hover:border-blue-100')">
            <div class="flex min-w-0 items-center gap-2.5">
              <Cloud :size="19" class="shrink-0" />
              <span class="truncate text-[13px] font-black">CDN</span>
            </div>
            <p class="mt-2.5 truncate text-[11px] font-mono opacity-60">备选，下载速度慢时使用</p>
          </button>
        </div>
      </section>

      <section class="space-y-5">
        <div class="flex items-center justify-between px-1">
          <h3 class="text-[13px] font-black text-slate-400 uppercase tracking-[0.2em]">整合包版本</h3>
          <button @click="loadManifestVersions(true)" :disabled="isCheckingManifest || isUpdatingPack"
                  class="text-[12px] font-black text-blue-600 hover:text-blue-700 uppercase disabled:opacity-30">
            {{ isCheckingManifest ? '检查中' : '刷新' }}
          </button>
        </div>

        <div class="rounded-[19px] border border-slate-100 bg-slate-50/50 p-6 space-y-5">
          <div class="grid grid-cols-2 gap-4">
            <div class="space-y-1.5 overflow-hidden">
              <p class="text-[11px] font-black text-slate-300 uppercase tracking-widest">本地</p>
              <p class="text-[14px] font-black text-slate-700 truncate">
                {{ manifestVersions?.local?.version || '未安装' }}
              </p>
              <p class="text-[11px] text-slate-400 font-mono truncate">
                {{ manifestVersions?.local?.manifestVersion || '-' }}
              </p>
            </div>
            <div class="space-y-1.5 overflow-hidden text-right">
              <p class="text-[11px] font-black text-slate-300 uppercase tracking-widest">远程</p>
              <p class="text-[14px] font-black text-slate-700 truncate">
                {{ manifestVersions?.remote.version || '-' }}
              </p>
              <p class="text-[11px] text-slate-400 font-mono truncate">
                {{ manifestVersions?.remote.manifestVersion || '-' }}
              </p>
            </div>
          </div>

          <Button v-if="manifestVersions?.needsUpdate" @click="handlePackUpdate" :disabled="isUpdatingPack"
                  class="w-full h-[53px] rounded-2xl bg-blue-600 hover:bg-blue-700 text-white text-[12px] font-black tracking-[0.2em]">
            <RefreshCw :size="17" :class="cn('mr-2.5', isUpdatingPack && 'animate-spin')" />
            {{ isUpdatingPack ? '跳转中' : '前往更新' }}
          </Button>
          <p v-else-if="manifestVersions" class="text-[12px] font-bold text-emerald-600 text-center py-2.5">
            已是最新版本
          </p>
          <p v-else class="text-[12px] font-bold text-slate-300 text-center py-2.5">
            等待检查
          </p>
        </div>
      </section>
    </div>

    <footer class="p-7 border-t border-slate-100 bg-white/80 flex-none">
      <Button variant="destructive"
              class="w-full py-8 bg-red-500 hover:bg-red-600 text-white rounded-[19px] font-black text-[14px] uppercase tracking-[0.2em] shadow-lg shadow-red-200/50 transition-all active:scale-[0.98]"
              @click="handleLogout">注销当前会话</Button>
    </footer>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, nextTick } from 'vue'
import { useRouter } from 'vue-router'
import { ArrowLeft as ArrowLeftIcon, Check, ChevronsUpDown, Cloud, Globe2, RefreshCw } from 'lucide-vue-next'
import { useCacheStore, type DownloadSource } from '@/stores/cache'
import { invoke } from "@tauri-apps/api/core"
import { toast } from 'vue-sonner'
import { cn } from '@/lib/utils'
import { open as openFileDialog } from '@tauri-apps/plugin-dialog'

import { Button } from '@/components/ui/button'
import { Slider } from '@/components/ui/slider'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Command, CommandEmpty, CommandGroup, CommandItem, CommandList } from '@/components/ui/command'
import { Badge } from "@/components/ui/badge"

const router = useRouter()
const cache = useCacheStore()

const triggerRef = ref<any>(null)
const triggerWidth = ref(0)
let resizeObserver: ResizeObserver | null = null

const updateWidth = () => {
  if (triggerRef.value?.$el) triggerWidth.value = triggerRef.value.$el.offsetWidth
}

const open = ref(false)
const isManualMem = ref(false)
const isTuning = ref(false)
const isCheckingManifest = ref(false)
const isUpdatingPack = ref(false)

const config = ref({
  javaPath: cache.settings?.javaPath || '',
  maxMemory: cache.settings?.maxMemory ?? 0,
  downloadSource: cache.settings?.downloadSource ?? 'r2'
})

const totalSystemMem = ref(cache.totalMemory)
const currentUsedMem = ref(4096)
const javaDetailList = ref(cache.javaList)
const sliderValue = ref([config.value.maxMemory])
const MEMORY_RESERVE_MB = 512
const MEMORY_STEP_MB = 512
const MEMORY_MIN_MB = 2048
const MEMORY_BASELINE_MAX_MB = 6144

const alignMemoryDown = (memoryMb: number) => Math.floor(memoryMb / MEMORY_STEP_MB) * MEMORY_STEP_MB
const autoMemoryRecommendation = (availableMb: number) => {
  const targetMb = availableMb <= MEMORY_BASELINE_MAX_MB
    ? availableMb
    : Math.max(MEMORY_BASELINE_MAX_MB, Math.floor(availableMb * 0.75))

  return Math.max(MEMORY_MIN_MB, alignMemoryDown(targetMb))
}

const selectedJava = computed(() => javaDetailList.value.find(j => j.path === config.value.javaPath))
const maxSafeMemory = computed(() => Math.max(MEMORY_STEP_MB, totalSystemMem.value - currentUsedMem.value - MEMORY_RESERVE_MB))
const otherUsedPercent = computed(() => (currentUsedMem.value / totalSystemMem.value) * 100)
const gamePercent = computed(() => (config.value.maxMemory / totalSystemMem.value) * 100)
const freeMemAfterAlloc = computed(() => Math.max(0, totalSystemMem.value - currentUsedMem.value - config.value.maxMemory))

interface ManifestInfo { version: string; manifestVersion: string }
interface ManifestVersions { local: ManifestInfo | null; remote: ManifestInfo; needsUpdate: boolean }
const manifestVersions = ref<ManifestVersions | null>(null)

const loadManifestVersions = async (forceRefresh = false) => {
  isCheckingManifest.value = true
  try {
    manifestVersions.value = await invoke<ManifestVersions>('get_manifest_versions', { forceRefresh })
  } catch (error) {
    toast.error("整合包版本检查失败: " + error, { duration: 10000 })
  } finally {
    isCheckingManifest.value = false
  }
}

const handlePackUpdate = async () => {
  isUpdatingPack.value = true
  await router.push('/')
}

const performAutoTune = async () => {
  try {
    const used = await invoke<number>('get_used_memory')
    currentUsedMem.value = used
    const available = Math.max(0, totalSystemMem.value - used - MEMORY_RESERVE_MB)
    const recommendation = autoMemoryRecommendation(available)
    config.value.maxMemory = recommendation
    sliderValue.value = [recommendation]
    return recommendation
  } catch (e) {
    return 4096
  }
}

const triggerSave = async (persistValue: number | null = null) => {
  const dataToSave = JSON.parse(JSON.stringify(config.value))
  if (persistValue !== null) dataToSave.maxMemory = persistValue

  cache.setSettings(dataToSave)
  try {
    await invoke('save_config', { config: dataToSave })
  } catch (e) {
    console.error(e)
  }
}

const handleJavaSelect = async (path: string) => {
  config.value.javaPath = path
  await triggerSave()
  open.value = false
}

const handleDownloadSourceSelect = async (source: DownloadSource) => {
  if (config.value.downloadSource === source) return
  config.value.downloadSource = source
  await triggerSave()
  toast.success(source === 'bitiful' ? "已切换至CDN" : "已切换至源站", { duration: 1500 })
}

const handleSliderChange = (val: number[] | undefined) => {
  const [newMemory] = val ?? []
  if (newMemory) config.value.maxMemory = newMemory
}

const handleManualToggle = async () => {
  if (isManualMem.value) {
    await triggerSave()
    toast.success("内存已保存", { duration: 1500 })
  }
  isManualMem.value = !isManualMem.value
}

const autoTuneMemory = async () => {
  isTuning.value = true
  try {
    await performAutoTune()
    await triggerSave(0)
    toast.success("已自动优化", { duration: 1500 })
  } finally {
    isTuning.value = false
  }
}

const handleLogout = async () => {
  cache.clearUser();
  await invoke('logout_current_user')
  await router.push('/')
}
interface JavaInfo { path: string; version: string }
onMounted(async () => {
  await nextTick()
  updateWidth()
  if (triggerRef.value?.$el) {
    resizeObserver = new ResizeObserver(updateWidth)
    resizeObserver.observe(triggerRef.value.$el)
  }

  try {
    currentUsedMem.value = await invoke<number>('get_used_memory')
  } catch (e) { }

  if (config.value.maxMemory === 0) {
    await performAutoTune()
  } else {
    sliderValue.value = [config.value.maxMemory]
  }

  invoke<JavaInfo[]>('scan_java_environments').then(async (list) => {
    if (list && list.length > 0) {
      cache.setJavaList(list);
    }
  });

  await loadManifestVersions(false)
})

onUnmounted(() => resizeObserver?.disconnect())

const handleManualImport = async () => {
  try {
    const isWindows = navigator.userAgent.includes('Windows');
    const targetName = isWindows ? 'java.exe' : 'java';

    const selected = await openFileDialog({
      multiple: false,
      directory: false,
      filters: [{
        name: 'Java Executable',
        // 根据平台过滤可执行文件
        extensions: isWindows ? (navigator.userAgent.includes('Windows') ? ['exe'] : []) : []
      }]
    });

    if (!selected || Array.isArray(selected)) return;
    const fileName = selected.split(/[\\/]/).pop()?.toLowerCase();

    if (fileName !== targetName) {
      toast.error(`该文件可能不是java`, { duration: 10000 });
      return;
    }
    const javaInfo: JavaInfo = await invoke('validate_java', { path: selected });

    const isExisted = javaDetailList.value.some(j => j.path === javaInfo.path);
    if (isExisted) {
      toast.error("该 Java 环境已存在", { duration: 10000 });
      return;
    }

    javaDetailList.value.push(javaInfo);
    handleJavaSelect(javaInfo.path);

  } catch (error) {
    // 错误处理：如果 validate_java 返回 Err，会进入这里
    toast.error("Java 验证失败: " + error, { duration: 10000 });
  }
}
</script>
