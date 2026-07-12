# 镜像代理配置指南 (Mirrors Configuration)

Left4Addons 支持配置自定义的请求镜像代理 (`mirrors.json`)。通过配置，你可以将应用发往特定域名（例如 Steam 创意工坊）的 HTTP/HTTPS 请求自动重定向到自定义的反向代理或者镜像节点。

## 配置文件位置

当你启动应用后，配置文件会自动读取自应用的运行时目录，通常位于应用的配置文件夹下。如果没有文件，可以手动新建一个 `mirrors.json`。

## 基本语法

`mirrors.json` 是一个包含多个**规则对象**的 JSON 数组。代理匹配支持三种模式：**对象模式**、**高级正则模式**、以及**简易字符串模式**。

### 字段说明 (通用)

- **`target`** (`String`, 必填)
  重定向到的目标域名。可以使用 `$1`, `$2` 等占位符来引用匹配规则中捕获的内容（例如被匹配到的子域名）。
  - 若为了保持向后兼容，如果你使用了简易字符串模式并保留了 `*` 号，`*` 仍然会被自动替换为第一个捕获组。
- **`keep-host`** (`Boolean`, 可选，默认 `false`)
  如果设置为 `true`，将在发送请求时自动添加一个 `Host` 请求头，值为被代理前的原始域名。这通常用于需要验证原始域名的反向代理。
- **`insecure-tls`** (`Boolean`, 可选，默认 `false`)
  仅当镜像的 HTTPS 证书不包含其自身域名、但服务端依赖原始 `Host` 时设为 `true`。这会跳过该规则对应请求的 TLS 证书校验，存在中间人攻击风险；应优先使用证书正确的镜像。
- **`headers`** (`Object`, 可选，默认无)
  允许在通过此镜像规则发起请求时，自动添加或覆盖特定的 HTTP 请求头。

---

### 模式一：对象模式 (推荐)
通过结构化的对象定义匹配规则，清晰且易于维护。

- **`domain`** (`String`, 必填)
  需要匹配的基础域名，例如 `"steamstatic.com"`。
- **`subdomains`** (`Array<String>`, 可选)
  需要匹配的子域名列表。例如 `["store", "media"]`。
  - 如果想要匹配任意子域名，可以使用 `["*"]`。
  - 子域名将被作为**第一个捕获组** `$1`。
- **`allow-www`** (`Boolean`, 可选，默认 `false`)
  是否允许可选的 `www.` 前缀。如果你希望 `www.steamstatic.com` 和 `steamstatic.com` 都能被匹配到，请设为 `true`。(`www.` 不会被捕获，不影响 `$1`)

**配置示例：**
```json
[
  {
    "domain": "steamstatic.com",
    "subdomains": ["store", "media"],
    "allow-www": true,
    "target": "$1.mirror.example.com"
  }
]
```
*以上规则可以匹配 `store.steamstatic.com` 和 `www.media.steamstatic.com`，重定向时 `$1` 会被替换为 `store` 或 `media`。*

---

### 模式二：简易字符串模式 (向下兼容)
通过一条字符串规则快速匹配，支持简单的分组和通配符。

- **`rules`** (`String`, 必填)
  - 组匹配：`[a,b].example.com`（相当于正则中的 `(a|b).example.com`），会作为捕获组 `$1`。
  - 可选组：`[www.]?example.com`，在 `]` 后面加上 `?` 表示该组为可选，这解决了以往匹配可选前缀时语法怪异的问题。
- 通配符匹配：`*`，匹配并捕获任何内容。
  - 可以用分号分隔多个完整规则，例如 `api.example.com;cdn.example.com`。
  - **提示**：在新版本中，建议在 `target` 中使用 `$1`, `$2` 来精确引用对应的捕获组，以避免 `[]` 组匹配意外抢占了通配符 `*` 的内容。

**配置示例：**
```json
[
  {
    "rules": "[www.]?*.steamcommunity.com",
    "target": "$2.steam.example.com",
    "keep-host": true
  }
]
```
*上述规则中，`[www.]?` 是第一个捕获组 `$1`（无论是否匹配上），而 `*` 则是第二个捕获组 `$2`。我们在 `target` 中明确使用了 `$2`，避免了冲突。*

---

### 模式三：高级正则模式
如果内置的匹配模式无法满足需求，你可以直接提供标准的正则表达式进行完全匹配控制。

- **`regex`** (`String`, 必填)
  标准正则表达式字符串。正则表达式将针对请求的完整 Host (包括端口，如果有的话) 进行匹配。

**配置示例：**
```json
[
  {
    "regex": "^(?:www\\.)?(.*?)\\.steampowered\\.com$",
    "target": "$1.proxy.my-server.com"
  }
]
```

## 其它配置示例

### 代理 Steam 创意工坊接口并保持 Host

```json
[
  {
    "rules": "api.steampowered.com",
    "target": "192.168.1.100:8080",
    "keep-host": true
  }
]
```

### 附加特定的 User-Agent

```json
[
  {
    "domain": "steampowered.com",
    "subdomains": ["store", "cdn"],
    "target": "$1.proxy.my-server.com",
    "headers": {
      "User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
      "X-Forwarded-For": "114.114.114.114"
    }
  }
]
```
