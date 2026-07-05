# Left4Addons (Left 4 Dead 2 Addons Manager) 🎮

<p align="center">
  <strong>一款基于 Tauri + React + TypeScript 打造的现代化、《求生之路 2》(Left 4 Dead 2) 创意工坊模组（Addons）管理器。</strong>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Developed%20with-AI%20(Antigravity)-blueviolet?style=for-the-badge&logo=google-gemini" alt="Developed with AI (Antigravity)">
  <img src="https://img.shields.io/badge/License-MIT-green?style=for-the-badge" alt="License">
  <img src="https://img.shields.io/badge/Platform-Windows-lightgrey?style=for-the-badge&logo=windows" alt="Platform">
</p>

---

## 🤖 AI 协同开发声明 (Developed with AI)

> [!IMPORTANT]
> **本项目的大部分核心代码、架构重构以及功能模块均由 AI 辅助/编写完成！**
> 
> 本项目是人类开发者 **akkariin** 与 Google DeepMind 团队研发的 Agentic AI 编码助手 **Antigravity** 深度对齐、结对编程的结晶。
> - **TypeScript 迁移**：AI 自动化将原有 JavaScript 代码平滑迁移至 TypeScript，并确立了严格的类型声明。
> - **Tauri 锈核架构**：由 AI 协助设计并编写高性能的 Rust 模块（如 VPK 解析器、文件同步与底层 IO 操作），实现前端与 native 底层的高效桥接。
> - **页面交互与重构**：UI 布局优化、模组管理、群组逻辑、以及批量重命名功能均通过 AI 迭代开发，保证了极佳的代码质量与执行效率。

---

## ✨ 核心特性

- 🚀 **高性能 VPK 解析**：使用 Rust 底层多线程扫描与分析 L4D2 模组（VPK 格式），秒级加载数百个模组文件。
- 📂 **模组分组与分类管理**：支持自定义标签与分组，按类别、功能或来源分类管理你的模组，告别混乱。
- 🔄 **Steam Workshop 深度同步**：一键同步 Steam 创意工坊数据，自动抓取模组名称、简介与高清预览图，并进行本地高性能缓存。
- ⚡ **一键启用/禁用**：直观的开关设计，底层无缝控制 VPK 状态，极速切换游戏模组配置。
- 🔍 **智能搜索与筛选**：支持按名称、分组、状态（启用/禁用）、创意工坊 ID 等多维度快速定位模组。
- ✍️ **批量重命名与整理**：解决模组重命名冲突，规范化模组文件名。
- 🎨 **现代化精致 UI**：采用暗色调、流畅微动画及毛玻璃感视觉设计，贴合玩家审美。

---

## 🛠️ 技术栈

- **Frontend**: React 19, Vite, TypeScript, Lucide Icons, Custom CSS Utilities (高性能响应式设计)
- **Backend/Native**: Tauri v2, Rust (底层的系统级文件处理、VPK 解析与缓存协议)
- **Linter**: Oxlint (超快的静态分析检查)

---

## 📥 快速开始

### 前提条件

你需要安装以下环境以进行本地开发或构建：

1. **Rust / Cargo** (可通过 [rustup](https://rustup.rs/) 安装)
2. **Node.js** (推荐 v18+)

### 安装依赖

```bash
npm install
```

### 启动开发环境

```bash
# 启动 Tauri 开发调试器（会同时运行前端 Vite 并拉起 Tauri 窗口）
npm run tauri:dev
```

### 构建打包

```bash
# 构建 Windows 下的生产版本可执行程序
npm run tauri:build
```

构建完成后，你可以在 `src-tauri/target/release/` 下找到生成的可执行文件。

---

## 📝 许可证

本项目基于 **[MIT License](file:///home/akkariin/文档/Left4Addons/LICENSE)** 开源。你可以自由地使用、修改和分发。

---

<p align="center">
  <em>Made with ❤️ and 🤖 (Antigravity AI Agent)</em>
</p>
