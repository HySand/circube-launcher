// stores/cache.ts
import { defineStore } from 'pinia'

interface JavaInfo { path: string; version: string }
export type DownloadSource = 'overseas' | 'chinaCdn'
export interface LauncherSettings { javaPath: string; maxMemory: number; downloadSource: DownloadSource }

const defaultSettings = (): LauncherSettings => ({
    javaPath: '',
    maxMemory: 4096,
    downloadSource: 'overseas',
})

export const useCacheStore = defineStore('cache', {
    state: () => ({
        user: null as {uuid: string ; name: string; accessToken: string; skinUrl: string; authType: string} | null,
        quote: { text: '', from: '' },
        settings: defaultSettings(),
        totalMemory: 0,
        javaList: [] as JavaInfo[]
    }),
    persist: false,
    actions: {
        setUser(user: {uuid: string ; name: string; accessToken: string; skinUrl: string; authType: string}) {
            this.user = user
        },
        setQuote(quote: { text: string, from: string }) {
            this.quote = quote
        },
        setSettings(settings: LauncherSettings) {
            this.settings = { ...defaultSettings(), ...settings }
        },
        clearUser() {
            this.user = null
        },
        setTotalMem(totalMem: number) {
            this.totalMemory = totalMem
        },
        setJavaList(javaList: JavaInfo[]) {
            this.javaList = javaList
        },
        clearQuote() {
            this.quote = { text: '', from: '' }
        },
        clearSettings() {
            this.settings = defaultSettings()
        }
    }
})
