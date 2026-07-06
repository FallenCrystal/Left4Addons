import React from 'react';
import i18n from 'i18next';
import { initReactI18next, useTranslation } from 'react-i18next';

const zh = {
  common: {
    confirm: "确定",
    cancel: "取消",
    save: "保存",
    saving: "正在保存...",
    create: "创建",
    creating: "正在创建...",
    rename: "重命名",
    renaming: "正在重命名...",
    loading: "加载中...",
    parsing: "正在解析 VPK 文件和加载数据中...",
    all: "全部",
    other: "其他",
    vpkCount: "{{count}} 个文件",
    emptyStateTitle: "未发现匹配的附加组件",
    emptyStateDesc: "在此筛选条件下未找到任何文件。请确认你的目录中已经放有 L4D2 附件文件。",
    rescan: "重新扫描",
    scanning: "正在扫描...",
    error: "错误",
    success: "成功"
  },
  categories: {
    All: "全部",
    Campaign: "战役",
    Survivor: "幸存者",
    "Weapon Model": "武器模型",
    Script: "脚本",
    Map: "地图",
    Skin: "皮肤",
    "Sound/Music": "声音/音乐",
    Infected: "感染者",
    "UI/Textures": "UI/贴图",
    Other: "其他"
  },
  sidebar: {
    title: "求生之路2附加组件管理",
    allAddons: "全部组件",
    manualInstall: "手动安装 (Addons)",
    workshopDir: "创意工坊目录 (Workshop)",
    disabledAddons: "被禁用的组件 (.disabled)",
    myGroups: "我的分组",
    noGroups: "暂无分组，可自动归类或手动创建。",
    newGroup: "新建手动分组",
    classifying: "归类中...",
    autoClassify: "自动识别战役并归类",
    settings: "设置"
  },
  topbar: {
    searchPlaceholder: "搜索组件名称, 描述, 作者或创意工坊 ID...",
    sortByTitle: "按名称排序",
    sortBySize: "按大小排序",
    sortById: "按创意工坊ID排序",
    exitBatch: "退出批量",
    batchManage: "批量管理",
    batchManageTooltip: "批量管理附件组件",
    refreshTooltip: "刷新所有组件并同步创意工坊信息"
  },
  addonCard: {
    deselect: "取消选择",
    select: "选中此组件",
    enabled: "已启用",
    disabled: "已禁用",
    manualInstall: "手动安装",
    workshop: "创意工坊",
    author: "作者: {{author}}",
    group: "分组: {{group}}",
    fileSize: "文件大小: {{size}}",
    containsFiles: "内含 {{count}} 个文件",
    clickToDisable: "点击禁用该附件",
    clickToEnable: "点击启用该附件",
    addOrRemoveGroup: "加入或移出群组",
    removeFromGroup: "从当前分组移出 ({{name}})",
    noGroupsTooltip: "无分组 (在侧栏创建)",
    openLink: "打开链接",
    openInSteam: "在 Steam 客户端打开",
    openInBrowser: "在浏览器中打开 (官方)",
    openInMirror: "在浏览器中打开 (国内镜像)",
    openBuiltInSource: "打开组件内置来源网页",
    moveToManual: "移动到手动安装目录 (Addons)",
    renameAddon: "重命名附件文件"
  },
  batchActionBar: {
    selectedCount: "已选择 {{count}} 个组件",
    deselectAll: "取消全选",
    selectAll: "全选",
    enable: "启用",
    disable: "禁用",
    moveToManual: "移动至手动安装",
    autoRename: "自动重命名",
    addToGroup: "加入分组",
    noGroups: "无分组",
    exitBatch: "退出批量"
  },
  groupCard: {
    partiallyEnabled: "部分启用",
    enabled: "已启用",
    disabled: "已禁用",
    groupBadge: "分组 ({{count}})",
    author: "作者: {{author}}",
    authorMultiple: "作者: {{first}} 等 {{count}} 位作者",
    containsAddons: "包含 {{count}} 个组件:",
    totalSize: "总大小: {{size}}",
    toggleAll: "启用/禁用此分组内所有组件",
    viewDetails: "查看分组详情",
    deselectGroup: "取消选择整个分组",
    selectGroup: "选中整个分组"
  },
  groupHeader: {
    desc: "此分组包含 {{count}} 个附件文件，可进行批量管理。",
    renameGroup: "重命名分组",
    deleteGroup: "解散分组",
    enableAll: "全部启用",
    disableAll: "全部禁用",
    moveToManualAll: "全部移动至手动安装"
  },
  groupModal: {
    title: "创建新附件分组",
    desc: "将多个 VPK 文件打包到同一个群组（例如一张地图的 Part 1、2、3），从而一键批量启用、禁用或移动。",
    groupNameLabel: "分组名称:",
    groupNamePlaceholder: "例如：Early Days 战役地图包",
    selectAddonsLabel: "选择要加入群组的组件 (可多选):",
    createGroupBtn: "创建分组"
  },
  editGroupModal: {
    title: "重命名分组",
    newNameLabel: "分组新名称:"
  },
  linkConfirmModal: {
    title: "即将访问外部网站",
    desc: [
      "您点击的链接指向第三方外部网页：",
      "请确保您信任该网站，以防止钓鱼和恶意软件威胁。是否继续前往？"
    ],
    continue: "继续访问"
  },
  renameModal: {
    title: "重命名附加组件文件",
    desc: "修改底层的 VPK 文件名称。建议使用有意义的标题，防止由于一堆数字无法辨认。",
    workshopTitle: "创意工坊标题:",
    applyWorkshopTitle: "应用创意工坊标题作为文件名",
    currentNameLabel: "当前文件名:",
    newNameLabel: "新文件名 (.vpk):"
  },
  confirmModal: {
    deleteGroupTitle: "确认删除分组",
    deleteGroupMsg: "确定要删除这个分组吗？这不会删除文件，只会解散群组。",
    batchRenameTitle: "批量自动重命名",
    batchRenameMsg: "确定要自动重命名选中的 {{count}} 个附件吗？系统将根据创意工坊标题 and 分组信息自动为它们命名。",
    batchMoveTitle: "移动创意工坊附件提示",
    batchMoveMsg: [
      "确定要将选中的 {{count}} 个创意工坊附件移动到手动安装目录吗？移动后建议前往 Steam 创意工坊取消订阅这些附件，否则 Steam 会在下次启动游戏时重新下载。",
      "...或者 前往 设置 > 实验性 > 创意工坊检测绕过",
      "在不取消订阅的情况下移动创意工坊物品。"
    ]
  },
  detailModal: {
    title: "附加组件详情",
    fileName: "文件名称",
    addonSize: "组件大小",
    filesInVpk: "包内文件",
    directory: "所在目录",
    manualInstall: "手动安装 (Addons)",
    workshop: "创意工坊 (Workshop)",
    currentStatus: "当前状态",
    enabled: "已启用 (加载中)",
    disabled: "已禁用",
    workshopId: "创意工坊 ID",
    relatedLink: "相关链接",
    clickToVisit: "点击访问",
    author: "作者: {{author}}",
    viewSteamProfile: "查看 Steam 档案",
    version: "版本: {{version}}",
    belongsToGroup: "所属分组: {{name}}",
    descriptionLabel: "组件描述:",
    openInSteam: "在 Steam 打开",
    webViewOfficial: "网页查看 (官方)",
    mirrorWebView: "国内网页镜像",
    visitSource: "访问来源网页",
    disableAddon: "禁用组件",
    enableAddon: "启用组件",
    moveToManual: "移动到手动安装"
  },
  moveWarningModal: {
    title: "移动创意工坊附件提示",
    veryImportant: "非常重要：",
    warningDesc: [
      "您正在将组件从<strong> 创意工坊目录 </strong>移动到<strong> 手动安装目录 (Addons) </strong>。",
      "移动后，<strong>请务必在 Steam 客户端或网页中“取消订阅”该组件！</strong>",
      "如果不取消订阅，每次您启动游戏时，Steam 客户端都可能会<strong>重新下载</strong>该组件，导致加载目录和创意工坊目录下同时存在两个相同的文件，引发资源冲突或重复加载。",
      "...或者 前往 设置 &gt; 实验性 &gt; 创意工坊检测绕过",
      "在不取消订阅的情况下移动创意工坊物品。"
    ],
    unsubscribeAndMove: "取消订阅并移动",
    moveDirectly: "直接移动"
  },
  workshopWarningModal: {
    title: "创意工坊附件已移动",
    successTitle: "自动转移成功",
    warningDesc: [
      "为了防止游戏重新下载覆盖，执行 <strong>{{actionName}}</strong> 操作前，已自动将相关的创意工坊附件移动至<strong>手动安装目录 (Addons)</strong>。",
      "请注意：",
      "您必须前往 Steam 创意工坊<strong>取消订阅</strong>此组件，否则游戏下次启动时仍会重复下载该附件并导致冲突！"
    ],
    dontShowAgain: "当前会话不再提醒此警告，并静默执行移动",
    iUnderstand: "我知道了",
    goToWorkshop: "前往创意工坊"
  },
  settings: {
    pathSettings: "路径设置",
    experimental: "实验性",
    about: "关于软件",
    language: "语言设置",
    languageTitle: "界面语言设置",
    languageDesc: "选择应用程序的用户界面显示语言。",
    title: "游戏与目录配置",
    desc: "请配置求生之路2的游戏附加组件目录（即 `addons` 文件夹）。程序将自动访问该文件夹及其下的 `workshop` 创意工坊文件夹，并解析和管理所有的 VPK 附件文件。",
    addonsPathLabel: "附加组件目录 (Addons 路径):",
    addonsPathPlaceholder: "例如: C:\\Program Files (x86)\\Steam\\steamapps\\common\\Left 4 Dead 2\\left4dead2\\addons",
    addonsPathHelp: "请选择游戏目录下的 `left4dead2/addons` 文件夹。保存后程序将自动开始扫描该目录。",
    saveAndRescan: "保存并重新扫描",
    savingAndScanning: "正在保存并扫描...",
    experimentalTitle: "实验性功能",
    experimentalDesc: "此处的选项处于实验性阶段。启用可能会对文件目录结构做出调整，请谨慎开启。",
    dummyBypassTitle: "创意工坊检测绕过",
    dummyBypassDesc: [
      "开启后，将 addon 从 workshop 移出时，将在 workshop 目录下自动生成一个 dummy addon（仅保留附件图片、原始 AppID 及版本号，将标题标记为原名 (L4A Dummy) 且说明改为由 L4A 生成），以试图绕过 L4D2 创意工坊订阅同步检测。",
      "请注意：创意工坊更新可能会导致新旧版本的 addon 同时加载并冲突。"
    ],
    aboutTitle: "关于 Left 4 Addons",
    aboutDesc: "一个专为《求生之路2》（Left 4 Dead 2）设计的附加组件（Addons）管理器。采用 Tauri + React + Rust 驱动，旨在为玩家提供极速、优雅的 VPK 文件管理体验。",
    featuresTitle: "主要功能",
    features: [
      "<b>一键启用/禁用</b>：快速重命名 `.vpk` 文件以在游戏中生效或失效。",
      "<b>创意工坊同步</b>：自动拉取并缓存创意工坊组件的封面图、标题 and 作者详情。",
      "<b>分组管理</b>：将多 Part 地图或关联组件组合，实现一键批量操作。",
      "<b>自动识别</b>：内置战役/地图包识别算法，自动检测并对关联附件进行重组。",
      "<b>物理隔离</b>：一键将工坊文件转移到本地加载文件夹，防止游戏联机订阅冲突。"
    ],
    licenseTitle: "开源许可",
    licenseDesc: "本项目遵循 MIT 协议开源。",
    languageLabel: "界面语言 (Language):"
  },
  settingsModal: {
    title: "设置附加组件加载路径"
  },
  statsBar: {
    totalCount: "总附件数",
    enabled: "已启用",
    disabled: "已禁用",
    totalDisk: "组件总大小"
  },
  toasts: {
    autoMoveFailed: "自动移动附件失败: {{err}}",
    dbRefreshSuccess: "数据库刷新成功",
    dataLoadFailed: "数据加载失败: {{err}}",
    addonEnabled: "附加组件已启用",
    addonDisabled: "附加组件已禁用",
    operationFailed: "操作失败: {{err}}",
    groupEnabled: "分组内所有组件已启用",
    groupDisabled: "分组内所有组件已禁用",
    moveSuccessLoading: "已移动到手动安装目录",
    moveSuccessWorkshop: "已移动到创意工坊目录",
    moveFailed: "移动失败: {{err}}",
    steamSyncSuccess: "Steam 同步成功",
    steamSyncFailed: "同步失败: {{err}}",
    autoClassifySuccess: "自动归类完成！已发现并组合了战役/地图包。",
    autoClassifyFailed: "自动归类失败: {{err}}",
    settingsSaveSuccess: "设置保存并扫描成功",
    settingsSaveFailed: "保存失败: {{err}}",
    renameSuccess: "重命名成功",
    renameFailed: "重命名失败: {{err}}",
    createGroupSuccess: "新建分组成功",
    createGroupFailed: "创建分组失败: {{err}}",
    deleteGroupSuccess: "分组已删除",
    deleteGroupFailed: "删除失败: {{err}}",
    renameGroupSuccess: "分组已重命名",
    removeFromGroupSuccess: "已移出该分组",
    addToGroupSuccess: "已加入分组",
    batchToggleSuccessEnable: "批量启用成功",
    batchToggleSuccessDisable: "批量禁用成功",
    batchMoveSuccess: "已成功批量移动到手动安装目录",
    batchRenameNoNeed: "选中的组件已是推荐文件名，无需重命名",
    batchRenameSuccess: "批量重命名成功 (重命名了 {{count}} 个文件)",
    batchAddGroupSuccess: "批量加入分组成功"
  }
};

const en = {
  common: {
    confirm: "Confirm",
    cancel: "Cancel",
    save: "Save",
    saving: "Saving...",
    create: "Create",
    creating: "Creating...",
    rename: "Rename",
    renaming: "Renaming...",
    loading: "Loading...",
    parsing: "Parsing VPK files and loading data...",
    all: "All",
    other: "Other",
    vpkCount: "{{count}} files",
    emptyStateTitle: "No matching addons found",
    emptyStateDesc: "No files found under this filter. Please make sure you have L4D2 addon files in your folder.",
    rescan: "Rescan",
    scanning: "Scanning...",
    error: "Error",
    success: "Success"
  },
  categories: {
    All: "All",
    Campaign: "Campaign",
    Survivor: "Survivor",
    "Weapon Model": "Weapon Model",
    Script: "Script",
    Map: "Map",
    Skin: "Skin",
    "Sound/Music": "Sound/Music",
    Infected: "Infected",
    "UI/Textures": "UI/Textures",
    Other: "Other"
  },
  sidebar: {
    title: "Left 4 Addons",
    allAddons: "All Addons",
    manualInstall: "Manual (Addons)",
    workshopDir: "Workshop (Workshop)",
    disabledAddons: "Disabled (.disabled)",
    myGroups: "My Groups",
    noGroups: "No groups yet. You can auto-classify or create manually.",
    newGroup: "New Manual Group",
    classifying: "Classifying...",
    autoClassify: "Auto-Identify & Classify",
    settings: "Settings"
  },
  topbar: {
    searchPlaceholder: "Search addon name, description, author or workshop ID...",
    sortByTitle: "Sort by Name",
    sortBySize: "Sort by Size",
    sortById: "Sort by Workshop ID",
    exitBatch: "Exit Batch",
    batchManage: "Batch Manage",
    batchManageTooltip: "Batch manage addon components",
    refreshTooltip: "Refresh all addons and sync workshop info"
  },
  addonCard: {
    deselect: "Deselect",
    select: "Select this addon",
    enabled: "Enabled",
    disabled: "Disabled",
    manualInstall: "Manual",
    workshop: "Workshop",
    author: "Author: {{author}}",
    group: "Group: {{group}}",
    fileSize: "Size: {{size}}",
    containsFiles: "Contains {{count}} files",
    clickToDisable: "Click to disable",
    clickToEnable: "Click to enable",
    addOrRemoveGroup: "Add to or Remove from Group",
    removeFromGroup: "Remove from Group ({{name}})",
    noGroupsTooltip: "No groups (create in sidebar)",
    openLink: "Open Link",
    openInSteam: "Open in Steam Client",
    openInBrowser: "Open in Browser (Official)",
    openInMirror: "Open in Browser (Mirror)",
    openBuiltInSource: "Open built-in source page",
    moveToManual: "Move to Manual (Addons)",
    renameAddon: "Rename Addon File"
  },
  batchActionBar: {
    selectedCount: "{{count}} addons selected",
    deselectAll: "Deselect All",
    selectAll: "Select All",
    enable: "Enable",
    disable: "Disable",
    moveToManual: "Move to Manual",
    autoRename: "Auto Rename",
    addToGroup: "Add to Group",
    noGroups: "No Groups",
    exitBatch: "Exit Batch"
  },
  groupCard: {
    partiallyEnabled: "Partially Enabled",
    enabled: "Enabled",
    disabled: "Disabled",
    groupBadge: "Group ({{count}})",
    author: "Author: {{author}}",
    authorMultiple: "Author: {{first}} and {{count}} others",
    containsAddons: "Contains {{count}} addons:",
    totalSize: "Total Size: {{size}}",
    toggleAll: "Enable/Disable all addons in group",
    viewDetails: "View Group Details",
    deselectGroup: "Deselect whole group",
    selectGroup: "Select whole group"
  },
  groupHeader: {
    desc: "This group contains {{count}} addon files, allowing batch management.",
    renameGroup: "Rename Group",
    deleteGroup: "Delete Group",
    enableAll: "Enable All",
    disableAll: "Disable All",
    moveToManualAll: "Move All to Manual"
  },
  groupModal: {
    title: "Create New Addon Group",
    desc: "Bundle multiple VPK files into the same group (e.g. Part 1, 2, 3 of a campaign map) for one-click batch operations.",
    groupNameLabel: "Group Name:",
    groupNamePlaceholder: "e.g. Early Days Campaign Pack",
    selectAddonsLabel: "Select addons to add (multi-select):",
    createGroupBtn: "Create Group"
  },
  editGroupModal: {
    title: "Rename Group",
    newNameLabel: "New Group Name:"
  },
  linkConfirmModal: {
    title: "About to visit external website",
    desc: [
      "The link you clicked points to a third-party external webpage:",
      "Please ensure you trust this website to prevent phishing and malware threats. Do you want to continue?"
    ],
    continue: "Continue"
  },
  renameModal: {
    title: "Rename Addon File",
    desc: "Modify the underlying VPK file name. Using meaningful titles is recommended to prevent unrecognizable filenames with long numbers.",
    workshopTitle: "Workshop Title:",
    applyWorkshopTitle: "Apply Workshop Title as Filename",
    currentNameLabel: "Current Filename:",
    newNameLabel: "New Filename (.vpk):"
  },
  confirmModal: {
    deleteGroupTitle: "Confirm Delete Group",
    deleteGroupMsg: "Are you sure you want to delete this group? This will not delete the files, only dissolve the group.",
    batchRenameTitle: "Batch Auto Rename",
    batchRenameMsg: "Are you sure you want to auto rename the selected {{count}} addons? The system will name them based on workshop title and group info.",
    batchMoveTitle: "Move Workshop Addon Warning",
    batchMoveMsg: [
      "Are you sure you want to move the selected {{count}} workshop addons to the manual folder? After moving, it is recommended to unsubscribe from them in Steam, otherwise Steam will redownload them on next startup.",
      "...or go to Settings > Experimental > Workshop Detection Bypass",
      "to move workshop items without unsubscribing."
    ]
  },
  detailModal: {
    title: "Addon Details",
    fileName: "File Name",
    addonSize: "Addon Size",
    filesInVpk: "Files in VPK",
    directory: "Directory",
    manualInstall: "Manual (Addons)",
    workshop: "Workshop (Workshop)",
    currentStatus: "Current Status",
    enabled: "Enabled (Loading)",
    disabled: "Disabled",
    workshopId: "Workshop ID",
    relatedLink: "Related Link",
    clickToVisit: "Click to visit",
    author: "Author: {{author}}",
    viewSteamProfile: "View Steam Profile",
    version: "Version: {{version}}",
    belongsToGroup: "Group: {{name}}",
    descriptionLabel: "Description:",
    openInSteam: "Open in Steam",
    webViewOfficial: "Web View (Official)",
    mirrorWebView: "Mirror Web View",
    visitSource: "Visit Source Webpage",
    disableAddon: "Disable Addon",
    enableAddon: "Enable Addon",
    moveToManual: "Move to Manual"
  },
  moveWarningModal: {
    title: "Move Workshop Addon Warning",
    veryImportant: "Very Important:",
    warningDesc: [
      "You are moving the addon from <strong>Workshop Directory</strong> to <strong>Manual Installation Directory (Addons)</strong>.",
      "After moving, <strong>please make sure to unsubscribe from this addon in Steam!</strong>",
      "If you do not unsubscribe, Steam may <strong>redownload</strong> this addon every time you launch the game, resulting in identical files existing in both directories, causing resource conflicts or duplicate loading.",
      "...or go to Settings > Experimental > Workshop Detection Bypass",
      "to move workshop items without unsubscribing."
    ],
    unsubscribeAndMove: "Unsubscribe & Move",
    moveDirectly: "Move Directly"
  },
  workshopWarningModal: {
    title: "Workshop Addon Moved",
    successTitle: "Auto Transfer Successful",
    warningDesc: [
      "To prevent the game from re-downloading and overwriting, before executing <strong>{{actionName}}</strong>, the related workshop addon has been automatically moved to <strong>Manual Installation Directory (Addons)</strong>.",
      "Please note:",
      "You must go to the Steam Workshop to <strong>unsubscribe</strong> from this addon, otherwise the game will duplicate download the addon on next startup, causing conflicts!"
    ],
    dontShowAgain: "Do not show this warning again in the current session, and silently move",
    iUnderstand: "I Understand",
    goToWorkshop: "Go to Workshop"
  },
  settings: {
    pathSettings: "Path Settings",
    experimental: "Experimental",
    about: "About",
    language: "Language",
    languageTitle: "Interface Language Settings",
    languageDesc: "Choose the display language for the application user interface.",
    title: "Game & Directory Configuration",
    desc: "Please configure your Left 4 Dead 2 game addons folder. The program will automatically access the addons folder and its workshop folder to parse and manage all VPK files.",
    addonsPathLabel: "Addon Folder Path (Addons path):",
    addonsPathPlaceholder: "e.g., C:\\Program Files (x86)\\Steam\\steamapps\\common\\Left 4 Dead 2\\left4dead2\\addons",
    addonsPathHelp: "Please select the `left4dead2/addons` directory. The program will scan the directory immediately after saving.",
    saveAndRescan: "Save and Rescan",
    savingAndScanning: "Saving and Scanning...",
    experimentalTitle: "Experimental Features",
    experimentalDesc: "The options here are in experimental phase. Enabling them may adjust the directory structure. Please use with caution.",
    dummyBypassTitle: "Workshop Detection Bypass",
    dummyBypassDesc: [
      "When enabled, moving an addon out of the workshop folder will automatically generate a dummy addon in the workshop directory (keeping only thumbnail, original AppID, version, prefixing name with (L4A Dummy) and marking generated by L4A) to attempt to bypass Steam Workshop synchronization checks.",
      "Please note: Workshop updates might cause duplicate loads and conflicts between old and new versions."
    ],
    aboutTitle: "About Left 4 Addons",
    aboutDesc: "A dedicated addons manager designed for Left 4 Dead 2. Powered by Tauri + React + Rust, it aims to provide players with a super fast and elegant VPK file management experience.",
    featuresTitle: "Key Features",
    features: [
      "<b>One-click Enable/Disable</b>: Quickly rename `.vpk` files to enable/disable them in game.",
      "<b>Workshop Sync</b>: Auto fetch and cache workshop cover photos, titles, and author details.",
      "<b>Group Management</b>: Combine multi-part maps or associated addons for one-click batch operations.",
      "<b>Auto Identification</b>: Built-in algorithm to identify campaigns/map packs and regroup them.",
      "<b>Physical Isolation</b>: One-click transfer of workshop files to local addons folder to prevent subscription conflicts."
    ],
    licenseTitle: "Open Source License",
    licenseDesc: "This project is open source under the MIT License.",
    languageLabel: "Interface Language:"
  },
  settingsModal: {
    title: "Configure Addon Directory Path"
  },
  statsBar: {
    totalCount: "Total Addons",
    enabled: "Enabled",
    disabled: "Disabled",
    totalDisk: "Total Addons Size"
  },
  toasts: {
    autoMoveFailed: "Auto move failed: {{err}}",
    dbRefreshSuccess: "Database refreshed successfully",
    dataLoadFailed: "Data load failed: {{err}}",
    addonEnabled: "Addon enabled",
    addonDisabled: "Addon disabled",
    operationFailed: "Operation failed: {{err}}",
    groupEnabled: "All addons in the group enabled",
    groupDisabled: "All addons in the group disabled",
    moveSuccessLoading: "Moved to manual addons directory",
    moveSuccessWorkshop: "Moved to workshop directory",
    moveFailed: "Move failed: {{err}}",
    steamSyncSuccess: "Steam sync successful",
    steamSyncFailed: "Steam sync failed: {{err}}",
    autoClassifySuccess: "Auto classification complete! Campaigns/map packs regrouped.",
    autoClassifyFailed: "Auto classification failed: {{err}}",
    settingsSaveSuccess: "Settings saved and directory rescanned successfully",
    settingsSaveFailed: "Save failed: {{err}}",
    renameSuccess: "Rename successful",
    renameFailed: "Rename failed: {{err}}",
    createGroupSuccess: "Group created successfully",
    createGroupFailed: "Create group failed: {{err}}",
    deleteGroupSuccess: "Group deleted",
    deleteGroupFailed: "Delete failed: {{err}}",
    renameGroupSuccess: "Group renamed successfully",
    removeFromGroupSuccess: "Removed from group",
    addToGroupSuccess: "Added to group",
    batchToggleSuccessEnable: "Batch enable successful",
    batchToggleSuccessDisable: "Batch disable successful",
    batchMoveSuccess: "Batch moved to manual addons directory successfully",
    batchRenameNoNeed: "Selected addons are already in recommended filename format, no rename needed",
    batchRenameSuccess: "Batch rename successful (renamed {{count}} files)",
    batchAddGroupSuccess: "Batch added to group successfully"
  }
};

i18n
  .use(initReactI18next)
  .init({
    resources: {
      zh: { translation: zh },
      en: { translation: en }
    },
    lng: localStorage.getItem('i18n_lang') || 'zh',
    fallbackLng: 'zh',
    interpolation: {
      escapeValue: false
    },
    returnObjects: true
  });

export default i18n;

// i18n Helper component to render multi-line descriptions/warnings securely using array notation (without using \n)
interface TransHTMLProps {
  i18nKey: string;
  values?: Record<string, any>;
  className?: string;
  style?: React.CSSProperties;
}

export const TransHTML: React.FC<TransHTMLProps> = ({ i18nKey, values, className, style }) => {
  const { t } = useTranslation();
  const text = t(i18nKey, values);

  if (Array.isArray(text)) {
    return (
      <div className={className} style={style}>
        {text.map((line: string, idx: number) => (
          <div key={idx} dangerouslySetInnerHTML={{ __html: line }} style={{ marginBottom: idx < text.length - 1 ? '8px' : '0' }} />
        ))}
      </div>
    );
  }

  return <span className={className} style={style} dangerouslySetInnerHTML={{ __html: text as string }} />;
};

interface TransParagraphsProps {
  i18nKey: string;
  values?: Record<string, any>;
}

export const TransParagraphs: React.FC<TransParagraphsProps> = ({ i18nKey, values }) => {
  const { t } = useTranslation();
  const text = t(i18nKey, values);

  if (Array.isArray(text)) {
    return (
      <>
        {text.map((line: string, idx: number) => (
          <React.Fragment key={idx}>
            {line}
            {idx < text.length - 1 && <br />}
          </React.Fragment>
        ))}
      </>
    );
  }

  return <>{text}</>;
};
