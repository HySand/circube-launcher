import { createApp } from 'vue'
import { createPinia } from 'pinia'
import piniaPluginPersistedstate from 'pinia-plugin-persistedstate'
import router from './router'
import App from './App.vue'
import './style.css'
import "vue-sonner/style.css"

const app = createApp(App)
const pinia = createPinia()

if (import.meta.env.PROD) {
    window.addEventListener('contextmenu', (e) => {
        e.preventDefault();
    }, false);
}
pinia.use(piniaPluginPersistedstate)
app.use(pinia)
app.use(router)
app.mount('#app')