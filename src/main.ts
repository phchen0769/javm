// 必须最先执行：在任何 invoke 发出前把 macOS 的 IPC 通道切到 postMessage（见该模块注释）
import '@/lib/ipcTransport'
import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import router from './router'
import { installAppLogging } from '@/lib/logging'
import '@/assets/index.css'
import 'vue-sonner/style.css'

const app = createApp(App)

await installAppLogging(app)

app.use(createPinia())
app.use(router)

app.mount('#app')
