import { createRouter, createWebHashHistory } from 'vue-router'
import Login from '../views/LoginView.vue'
import Main from '../views/MainView.vue'
import Settings from '../views/SettingsView.vue'
import Loading from '../views/LoadingView.vue'

const routes = [
    { path: '/login', name: 'login', component: Login },
    { path: '/main', name: 'main', component: Main },
    { path: '/settings', name: 'settings', component: Settings },
    { path: '/', name: 'loading', component: Loading },
]

const router = createRouter({
    history: createWebHashHistory(),
    routes,
})

export default router