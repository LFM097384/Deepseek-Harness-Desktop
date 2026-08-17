# DeepSeek Harness Desktop

DeepSeek Harness 的 Tauri 2 桌面壳（Windows / macOS / Linux）。

- 启动时自动拉起官方 `dsh web`，窗口内嵌 Web UI（默认 `http://127.0.0.1:3080`）
- 幂等预装官方社区插件市场 `dshmarket`：Web UI 的 Settings → Plugin Market 可浏览 800+ 插件并一键安装（安装失败自动以 npmmirror 镜像重试，不阻塞启动）
- 品牌化加载页：分阶段状态（环境检查 / 插件市场 / 启动后端 / 连接服务）+ 错误诊断
- 退出时自动清理 `dsh` 进程树；若 3080 端口已有实例则直接复用

## 要求

- Node.js + 全局安装的 `@deepseek-ai/dsh`（`npm install -g @deepseek-ai/dsh`）
- Rust（Tauri 2 系统依赖）

## 开发

```sh
cargo build
# 或带热重载:
cargo tauri dev
```

## 打包

```sh
npx tauri build   # 产出 NSIS 安装包（target/release/bundle/nsis/）
```

## 环境变量

| 变量 | 用途 |
| --- | --- |
| `DSH_DESKTOP_DSH_CMD` | 覆盖 dsh 可执行命令（默认 `dsh`） |
| `DSH_HOME` | 覆盖 DSH 数据目录（默认 `~/.dsh`） |
| `npm_config_registry` | pnpm/npm 源（影响插件市场安装） |
