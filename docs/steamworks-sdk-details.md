# Steamworks SDK 细节与限制

本文档记录 Left 4 Addons 当前接入 Steamworks SDK 时已经确认的行为、限制和取舍. 

## 1. 总结

- Steamworks SDK 的查询结果与 Steam Community 网页端并不等价, 目前无法仅依靠 SDK 实现高可用性的精准搜索. 
- L4A (Left 4 Addons) 可通过 Steamworks SDK 触发 Steam 下载附件, 但过程不可靠. 这也是为何我们要尽可能回退到“通过 URL 下载”方案的原因之一. 

## 2. 搜索行为限制

### 2.1 SDK 搜索不等同于网页搜索

当前使用的 SDK 搜索基于 `ISteamUGC` 文本检索能力. 实测它和网页端 `steamcommunity.com/workshop/browse` 的语义差异很大:

多词查询经常劣化为宽松匹配 (逻辑近似 OR, 而不是网页端符合预期的短语或相关性匹配):
  - 搜索 `Early Days`
  - 网页端可返回 `Early Days` 战役相关 `Part 1-6`
  - SDK 侧则可能返回 `Early 2008 style`、`Early Zombie`、`Day Break`、`Release Day Bots`

单词查询结果同样可能严重偏离:
  - 搜索 `Cambalache`
  - 预期结果是 `Cambalache 2`
  - SDK 结果里完全没有相关附件


SDK 文本搜索目前完全无法复刻网页端的搜索体验. 如果需要“像网页那样”的精准搜索, 仍必须依赖网页抓取或混合方案. 

### 2.2 分类浏览无法纯 SDK 完整复原

当前分类浏览依赖 Steam Community 首页 SSR 数据里的 `declared_tags_v5` 等页面信息. 

- “分类树/分类浏览”目前是网页端或 SDK+HTML 混合能力
- 截止目前, 仅靠纯 Steamworks SDK 不可能实现该功能. 

## 3. 下载行为限制

### 3.1 文件格式与较新创意工坊物品不同

对于较新的游戏 (如 Wallpaper Engine) , 创意工坊物品通常是一个包含多个文件的文件夹 (甚至涉及更复杂的着色器编译等) . 

但对 L4D2 而言, Workshop 物品由游戏自身处理并安装到 `steamapps/common/Left 4 Dead 2/left4dead2/addons/workshop/` 目录下. 

通过 SDK 触发的下载产物位于:

`steamapps/workshop/content/550/<workshop-id>/<longint>_legacy.bin`

> 注: _legacy.bin 似乎是 Steam 针对部分较老的单文件 (如起源引擎用的 `.vpk`) 的重命名机制,   
> 但具体细节仍不透明.   
> 尽管将 _legacy.bin 手动改名为 .vpk 后能够正常工作, 但这已经是所有下载问题中最小的一个. 

### 3.2 Steam 下载进度不可靠

Steam客户端的下载管理会出现非真实状态的情况:

- Steam 下载管理里长时间显示 0 进度 / 0 网络 / 0 磁盘
- 可能会进入“正在停止”
- 没有稳定可见的进度条
- 突然显示已完成

虽然 L4A 内部获取的进度看起来正常, 但无法保证底层确实在正常工作.

### 3.3 无法重新下载或只能保留两个副本

- 如果直接删除 `steamapps/workshop/content/550/...` 下的文件夹或里面的文件,  
  再次请求后 Steam 也不会重新下载.
- 取消订阅后退出游戏进程 (无论是L4A还是求生之路 即游戏退出未运行的状态时),   
  Steam 不会删除目标文件/文件夹.
- Steam 可能还持久化保存了如 Manifest 状态.  
  虽怀疑是 `steamapps/workshop/appworkshop_550.acf` 作祟,  
  但删除后重启 Steam 仍然不起作用  
  这是 L4A 无法稳定通过 SDK 下载附件它的核心阻碍之一.

在 Wallpaper Engine 侧测试时发现,   
如果在 Steam 自动删除前手动删除了物品缓存 (无论是否已订阅) 都会导致严重状态错误.   
最致命的影响之一是永久无法再次订阅该物品  
(除非去网页端订阅, 或换一台计算机操作, 但后续行为暂未测试, 具体后续行为也是个未知数).

## 4. Steamworks SDK 无法直接获取下载 URL

SDK 里确实有 `ISteamRemoteStorage::GetPublishedFileDetails`

> `Deprecated - Only used with the deprecated RemoteStorage based Workshop API`

这是旧版 RemoteStorage API 的接口，且其返回值中并没有暴露文件的直接下载链接。

虽然它看起来和网页端免鉴权的 Web API  
即 (https://api.steampowered.com/ISteamRemoteStorage/GetPublishedFileDetails/v1/)  
名字几乎一样, 但 SDK 侧偏偏缺少了最核心的 `file_url` 字段,   
导致无法在不额外请求网络 API 的情况下得到下载链接.

## 5. 从旧README中迁移过来的已知缺点

- 使用 Steamworks SDK 会使 Left 4 Addons 在 Steam 状态中“伪装”成求生之路, 以此来获取访问创意工坊的权限.
- **VAC 信任问题**:  
  如果你在使用 L4A 调用 SDK 时启动游戏. 会导致游戏本体短暂不被 VAC 信任.  
  这不会导致封号，但你必须彻底退出 L4A 后再启动游戏才能恢复正常。
- 仅靠 SDK 无法满足现有需求, 但某些加速器只给加速 Steam 相关进程. 求生之路本体都是自己下的Addon 那些加速器那就是啥用没有 挂个梯子吧.
- 只要 L4A 调用过一次 SDK, Steam 就会一直显示你“正在玩求生之路”, 直到 L4A 进程完全结束.   
  如果你在 Steam 库里好奇点了一下“停止”, L4A 就会被关掉.
- 由于 L4D2 创意工坊的 Addon 是游戏内/第三方直连下载  
  市面上那些仅加速特定进程的加速器对此毫无作用.  
  建议根据自身网络环境决定是否禁用 Steamworks SDK, 或直接使用系统级代理工具.

## 后记

我要写点有人味的东西, 这样你才不会觉得太枯燥(或许) 或者完全是用 AI 生成的垃圾什么的.

清理了一下 GPT 生成的无意义描写. 依旧给GPT语擦屁股. 虽然我叫它狗屁通 但写出来的东西也不完全是狗屁不通. 跑分还怪高的嘞

没有网页版的创意工坊简直就是地狱, 这个 SDK 的搜索逻辑 ——小红车用户自适应太牛逼了.

写了这么多缺点, Steamworks SDK 也并非没有好处. 相反效果还挺好的, 只是明显只靠它不是最佳解决方案.

Steamworks SDK 特色功能:
- (几乎) 无速率限制的 API 请求. 或者 Steam 自己已经帮我们处理好了这点
- 不需要爬网站就可以解析创意工坊物品的依赖 (这样不会战役Part冲突就瞎几把报“假阳性"了)
- 我可以一把抓到你的所有创意工坊订阅 (但是我好像拉不到收藏的合集 但自己导入L4A其实也很简单了)

你别看我现在就写出来这3个 其实算是很强大的了 因为我总不可能爬几百个附件对应的网站.  
也可以其实, 那 Too many request 伺候就你就不嘻嘻了.

希望 V 社越做越好吧. 别跟我说史山的事, 至少我希望之后 SDK 和创意工坊相关的逻辑不会这么屎. 大概多久? 可能是 2028 年吧. 2028 年应该有足够强大的模型给史山擦屁股了.

最近几周创意工坊网页更新很棒. SDK 只要能做一样的事那算是祖坟冒青烟了.
