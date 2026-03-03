<template>
  <div class="h-full flex flex-col bg-white overflow-hidden select-none">
    <header class="p-6 flex items-center border-b border-slate-100 flex-none">
      <Button variant="ghost" size="icon" @click="router.back()" class="mr-2 rounded-full text-slate-400 hover:bg-slate-50 transition-transform active:scale-90">
        <ArrowLeftIcon :size="20" />
      </Button>
      <h2 class="text-lg font-black text-slate-800 uppercase tracking-tighter">设置</h2>
    </header>

    <div class="flex-1 overflow-y-auto p-6 space-y-10 custom-scroll">
      <section class="space-y-4">
        <h3 class="text-[10px] font-black text-slate-400 uppercase tracking-[0.2em]">JDK选择</h3>
        <Popover v-model:open="open">
          <PopoverTrigger as-child>
            <Button ref="triggerRef" variant="outline" role="combobox" class="w-full justify-between h-auto py-4 px-5 border-slate-100 rounded-2xl bg-slate-50/50 hover:bg-white hover:border-blue-100 transition-all shadow-sm">
              <div class="flex flex-col items-start gap-1.5 overflow-hidden text-left">
                <span class="text-[11px] font-black tracking-widest text-blue-600">
                  {{ selectedJava?.version || (javaDetailList.length > 0 ? '请选择 Java 环境' : '未检测到 Java') }}
                </span>
                <span class="text-[9px] text-slate-400 truncate font-mono opacity-60 w-full">
                  {{ selectedJava?.path || '等待后端扫描...' }}
                </span>
              </div>
              <ChevronsUpDown class="ml-2 h-4 w-4 shrink-0 opacity-30 text-slate-400" />
            </Button>
          </PopoverTrigger>
          <PopoverContent class="p-1 border border-slate-10 rounded-2xl shadow-2xl bg-white/20 backdrop-blur-md" :style="{ width: `${triggerWidth}px`, minWidth: `${triggerWidth}px` }" align="start" :side-offset="8">
            <Command class="bg-transparent">
              <CommandList class="max-h-[280px] overflow-y-auto custom-scroll">
                <CommandEmpty class="py-10 text-[10px] text-center font-bold text-slate-300 tracking-widest">系统内未发现可用的 Java 环境</CommandEmpty>
                <CommandGroup>
                  <CommandItem v-for="java in javaDetailList" :key="java.path" :value="java.path" @select="handleJavaSelect(java.path)" class="flex items-center justify-between p-3 mb-1 rounded-xl cursor-pointer hover:bg-slate-50 group">
                    <div class="flex flex-col gap-1 overflow-hidden">
                      <span class="font-black text-[11px] tracking-tight text-slate-700 group-hover:text-blue-600 transition-colors">{{ java.version }}</span>
                      <span class="text-[9px] text-slate-400 truncate font-mono opacity-50">{{ java.path }}</span>
                    </div>
                    <Check :class="cn('ml-auto h-4 w-4 text-blue-600 transition-all', config.javaPath === java.path ? 'opacity-100 scale-100' : 'opacity-0 scale-50')" />
                  </CommandItem>
                </CommandGroup>
              </CommandList>
            </Command>
          </PopoverContent>
        </Popover>
      </section>

      <section class="space-y-6">
        <div class="flex items-center justify-between px-1">
          <h3 class="text-[10px] font-black text-slate-400 uppercase tracking-[0.2em]">内存调优</h3>
          <div class="flex gap-4">
            <button @click="autoTuneMemory" :disabled="isTuning" class="text-[10px] font-black text-blue-600 hover:text-blue-700 uppercase disabled:opacity-30">
              {{ isTuning ? '探测中' : '自动优化' }}
            </button>
            <button @click="handleManualToggle" class="text-[10px] font-black uppercase transition-colors" :class="isManualMem ? 'text-emerald-600' : 'text-slate-300 hover:text-slate-400'">
              {{ isManualMem ? '保存' : '手动' }}
            </button>
          </div>
        </div>

        <div class="relative pt-2 px-1">
          <div class="relative h-2 w-full bg-slate-100 rounded-full overflow-hidden shadow-inner">
            <div class="absolute left-0 top-0 h-full bg-blue-700 transition-all duration-1000" :style="{ width: `${otherUsedPercent}%` }" />
            <div class="absolute top-0 h-full bg-blue-500 shadow-[0_0_12px_rgba(59,130,246,0.4)] transition-all duration-500" :style="{ left: `${otherUsedPercent}%`, width: `${gamePercent}%` }" />
          </div>
        </div>

        <div class="grid grid-cols-3 gap-2 px-1">
          <div class="flex flex-col gap-2">
            <Badge variant="secondary" class="w-fit bg-slate-100 text-blue-700 hover:bg-slate-100 px-1.5 py-0 text-[8px] font-black tracking-[0.1em] rounded-md shadow-none border-none">系统内存</Badge>
            <span class="text-blue-800 font-numeric font-bold text-[11px] ml-0.5">{{ (totalSystemMem / 1024).toFixed(1) }} <small class="opacity-50">GB</small></span>
          </div>

          <div class="flex flex-col items-center gap-2">
            <Badge class="bg-blue-50 text-blue-600 hover:bg-blue-50 px-1.5 py-0 text-[8px] font-black tracking-[0.1em] rounded-md shadow-none border-none">游戏分配</Badge>
            <span class="text-blue-700 font-numeric font-bold text-[11px]">{{ config.maxMemory }} <small class="opacity-50">MB</small></span>
          </div>

          <div class="flex flex-col items-end gap-2">
            <Badge variant="secondary" class="bg-green-50/50 text-green-600 border-green-100 px-1.5 py-0 text-[8px] font-black tracking-[0.1em] rounded-md shadow-none">剩余内存</Badge>
            <span class="text-slate-600 font-numeric font-bold text-[11px] mr-0.5">{{ (freeMemAfterAlloc / 1024).toFixed(1) }} <small class="opacity-50">GB</small></span>
          </div>
        </div>

        <transition enter-active-class="transition duration-300 ease-out" enter-from-class="opacity-0 -translate-y-2">
          <div v-if="isManualMem" class="px-1 py-2">
            <Slider v-model="sliderValue" :min="512" :max="maxSafeMemory" :step="512" class="[&_[role=slider]]:bg-blue-600 [&_[role=slider]]:border-blue-600 [&_.relative_span:first-child]:bg-blue-600/20 [&_[data-orientation=horizontal]\_span:first-child]:bg-blue-600" @update:model-value="handleSliderChange" />
          </div>
        </transition>
      </section>
    </div>

    <footer class="p-6 border-t border-slate-100 bg-white/80 backdrop-blur-md flex-none">
      <Button variant="destructive" class="w-full py-7 bg-red-500 hover:bg-red-600 text-white rounded-2xl font-black text-[11px] uppercase tracking-[0.2em] shadow-lg shadow-red-200/50 transition-all active:scale-[0.98]" @click="handleLogout">注销当前会话</Button>
    </footer>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, nextTick } from 'vue'
import { useRouter } from 'vue-router'
import { ArrowLeft as ArrowLeftIcon, Check, ChevronsUpDown } from 'lucide-vue-next'
import { useCacheStore } from '@/stores/cache'
import { invoke } from "@tauri-apps/api/core"
import { toast } from 'vue-sonner'
import { cn } from '@/lib/utils'

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

const config = ref({
  javaPath: cache.settings?.javaPath || '',
  maxMemory: cache.settings?.maxMemory ?? 0
})

const totalSystemMem = ref(cache.totalMemory)
const currentUsedMem = ref(4096)
const javaDetailList = ref(cache.javaList)
const sliderValue = ref([config.value.maxMemory])

const selectedJava = computed(() => javaDetailList.value.find(j => j.path === config.value.javaPath))
const maxSafeMemory = computed(() => Math.max(512, totalSystemMem.value - currentUsedMem.value - 512))
const otherUsedPercent = computed(() => (currentUsedMem.value / totalSystemMem.value) * 100)
const gamePercent = computed(() => (config.value.maxMemory / totalSystemMem.value) * 100)
const freeMemAfterAlloc = computed(() => Math.max(0, totalSystemMem.value - currentUsedMem.value - config.value.maxMemory))

const performAutoTune = async () => {
  try {
    const used = await invoke<number>('get_used_memory')
    currentUsedMem.value = used
    const recommendation = Math.max(2048, Math.floor((totalSystemMem.value - used - 512) * 0.75 / 512) * 512)
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

const handleSliderChange = (val: number[] | undefined) => {
  const [newMemory] = val ?? []
  if (newMemory) config.value.maxMemory = newMemory
}

const handleManualToggle = async () => {
  if (isManualMem.value) {
    await triggerSave()
    toast.success("内存已保存", { duration: 1000 })
  }
  isManualMem.value = !isManualMem.value
}

const autoTuneMemory = async () => {
  isTuning.value = true
  try {
    await performAutoTune()
    await triggerSave(0)
    toast.success("已自动优化", { duration: 1000 })
  } finally {
    isTuning.value = false
  }
}

const handleLogout = async () => { cache.clearUser(); router.push('/main') }

onMounted(async () => {
  await nextTick()
  updateWidth()
  if (triggerRef.value?.$el) {
    resizeObserver = new ResizeObserver(updateWidth)
    resizeObserver.observe(triggerRef.value.$el)
  }

  try {
    currentUsedMem.value = await invoke<number>('get_used_memory')
  } catch (e) {}

  if (config.value.maxMemory === 0) {
    await performAutoTune()
  } else {
    sliderValue.value = [config.value.maxMemory]
  }
})

onUnmounted(() => resizeObserver?.disconnect())
</script>