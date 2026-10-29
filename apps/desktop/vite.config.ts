import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// Tauri 开发时前端跑在固定端口 1420,并由 tauri.conf.json 的 devUrl 接管。
// TAURI_DEV_HOST 用于手机/局域网真机调试(§7 构建与发布)。
const host = process.env.TAURI_DEV_HOST

export default defineConfig({
  plugins: [react()],
  // 保留 Tauri 自己的控制台输出,便于看 IPC 错误
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 1421 } : undefined,
    watch: {
      // src-tauri 归 Rust 工具链管,不要让 Vite 的 watcher 去扫 target/
      ignored: ['**/src-tauri/**'],
    },
  },
  envPrefix: ['VITE_', 'TAURI_ENV_*'],
  build: {
    // WebView2(Windows 10/11 内置)最低支持 Chromium 110
    target: 'chrome110',
    // Vite 8(rolldown)自带 oxc 压缩器;显式指定 'esbuild' 需要额外安装 esbuild,
    // 这里用布尔值走 Vite 的默认压缩器。
    minify: !process.env.TAURI_ENV_DEBUG,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    outDir: 'dist',
    emptyOutDir: true,
  },
})
