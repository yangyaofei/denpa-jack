import { resolve } from "path";
import { defineConfig } from "vite";

export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // cargo build 产物在项目树内, 触发 vite 无限 page reload(截图裸 HTML 畸形元凶)
      ignored: ["**/src-tauri/target/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "chrome105",
    minify: !process.env.TAURI_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_DEBUG,
    rollupOptions: {
      input: {
        main: resolve(__dirname, "index.html"),
        hud: resolve(__dirname, "hud.html"),
      },
    },
  },
});
