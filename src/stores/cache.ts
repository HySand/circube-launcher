// stores/cache.ts
import { defineStore } from 'pinia'

interface JavaInfo { path: string; version: string }
export const useCacheStore = defineStore('cache', {
    state: () => ({
        user: null as {uuid: string ; name: string; accessToken: string; skinUrl: string; authType: string} | null,
        quote: { text: '', from: '' },
        settings: {
            javaPath: '',
            maxMemory: 4096,
        },
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
        setSettings(settings: { javaPath: string, maxMemory: number }) {
            this.settings = settings
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
            this.settings = { javaPath: '', maxMemory: 4096 }
        }
    }
})