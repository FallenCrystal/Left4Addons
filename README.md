<div style="text-align: center;">

  # Left 4 Addons  

(可能是) 强大的《求生之路 2》(Left 4 Dead 2) 创意工坊模组 (Addons) 管理器。
</div>

<div style="text-align: center;">
  <img src="https://img.shields.io/badge/Developed%20with-AI%20(Antigravity)-blueviolet?style=for-the-badge&logo=google-gemini" alt="Developed with AI (Antigravity)">
  <img src="https://img.shields.io/badge/License-MIT-green?style=for-the-badge" alt="License">
</div>

![Screenshot](/preview/2026-07-05-224321.png)

> 佩萨罗 (Pesaro) 和 切尔诺贝利 (Chernobyl) 封面可能含有令人不适的内容.  
> 对此处理造成的观感问题, 十分抱歉.

---

## 生成式 AI 声明 (Developed with AI)

> [!IMPORTANT]
> **本项目的大部分核心代码 架构重构以及功能模块均由 AI 辅助/编写完成.**
> 
> Antigravity 并没有我想象中的那么难用, 但是就想闲暇之余蹬一下额度而已.  
> 我只做了找bug和UI/UX反馈, 没有对其进行代码Review.  
> 环境为 Antigravity + Gemini (3.5 Flash High & 3.1 Pro High)

> [!WARNING]
> 
> UI 可能会出现错位/错误遮挡等问题. 对此造成的体验不佳万分抱歉.  
> 可以通过打开问题或提交 PR 来解决.

---

## 特色功能

- 战役 (Campaign) Part 合并: 支持自动检测将分散的战役 Part 文件合并为一组.
- (批量) 重命名: 自动重命名为可读名称, 避免在创意工坊 ID 列表里大海捞针
- 快速移动: 将想要的附加组件从创意工坊内移动到 `addons` 文件夹下.   
  (但是不支持自动取消订阅)
- 快速切换: 快速启用或禁用特定的附加组件.  
  (不支持还未移动到 `addons` 文件夹下的组件)
- Dummy Bypass (实验性): 通过一个空的合法 `.vpk` 文件,  
  允许在不取消订阅的情况下将附件移出创意工坊并绕过创意工坊更新检查.

## 局限性
- Dummy Bypass:
  - 创意工坊更新时 Dummy 可能会被替换掉更新后的真实附件.
  - L4D2 本身有bug可能会导致触发全量重新下载
  - 游戏内的附加内容会看到两份相同的附件   
    一份是创意工坊名 一份是附件内 `addoninfo.txt` 的名  
    尽管创意工坊侧的 Dummy 附件不添加任何游戏内容.

---

## 技术栈

- **Frontend**: React 19, Vite, TypeScript, Lucide Icons
- **Backend/Native**: Tauri v2, Rust
- **Linter**: Oxlint

---

## 快速开始

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
# 构建生产版本可执行程序
npm run tauri:build
```

构建完成后，你可以在 `src-tauri/target/release/` 下找到生成的可执行文件。

### Linux 上交叉构建 Windows 版本

如果你当前在 Linux 上开发，推荐先使用 `x86_64-pc-windows-gnu` 目标：

```bash
rustup target add x86_64-pc-windows-gnu
npm run tauri:build:win-gnu
```

构建完成后，Windows 产物位于 `src-tauri/target/x86_64-pc-windows-gnu/release/`。

其中 Steamworks bridge 运行时文件会自动放到 `src-tauri/target/x86_64-pc-windows-gnu/release/steam/`：

- `l4a-steam-bridge.dll`
- `steam_api64.dll`

这样无需再手动复制 Steam bridge 相关 DLL。

---

## 许可证

本项目基于 **[MIT License](file:///home/akkariin/文档/Left4Addons/LICENSE)** 开源。你可以自由地使用、修改和分发。
