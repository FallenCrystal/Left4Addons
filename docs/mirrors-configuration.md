# 镜像代理配置指南 (Mirrors Configuration)

Left4Addons 支持配置自定义的请求镜像代理 (`mirrors.json`)。通过配置，你可以将应用发往特定域名（例如 Steam 创意工坊）的 HTTP/HTTPS 请求自动重定向到自定义的反向代理或者镜像节点。

## 配置文件位置

当你启动应用后，配置文件会自动读取自应用的运行时目录，通常位于应用的配置文件夹下。如果没有文件，可以手动新建一个 `mirrors.json`。

## 基本语法

`mirrors.json` 是一个包含多个**规则对象**的 JSON 数组。

```json
[
  {
    "rules": "[store,media].steampowered.com",
    "target": "*.steam.example.com",
    "keep-host": true,
    "headers": {
      "User-Agent": "MyCustomAgent/1.0",
      "X-Custom-Auth": "SecretToken"
    }
  }
]
```

### 字段说明

- **`rules`** (`String`, 必填)
  用于匹配原始请求的域名的规则表达式。
  - 支持类似 `[a,b].example.com` 的组匹配（相当于正则中的 `(a|b).example.com`）。
  - 支持 `*` 作为通配符匹配（捕获任何内容）。

- **`target`** (`String`, 必填)
  重定向到的目标域名。
  - 若 `rules` 中使用了 `*` 通配符或者 `[...]` 组匹配，匹配到的内容会自动替换 `target` 中的 `*`。例如：`rules: "[store,media].steampowered.com"` 匹配到了 `store.steampowered.com`，则 `target: "*.steam.example.com"` 会被替换为 `store.steam.example.com`。

- **`keep-host`** (`Boolean`, 可选，默认 `false`)
  如果设置为 `true`，将在发送请求时自动添加一个 `Host` 请求头，值为被代理前的原始域名。这通常用于需要验证原始域名的反向代理，或特定的 SNI 转发。

- **`headers`** (`Object`, 可选，默认无)
  允许在通过此镜像规则发起请求时，自动添加或覆盖特定的 HTTP 请求头（Headers）。可以用它来提供访问令牌、更改 `User-Agent` 或 `Referer` 等等。配置为标准的键值对（Key-Value）形式。

## 配置示例

### 1. 代理 Steam 创意工坊接口并保持 Host

将创意工坊的请求指向自建的代理服务器：

```json
[
  {
    "rules": "api.steampowered.com",
    "target": "192.168.1.100:8080",
    "keep-host": true
  }
]
```

### 2. 泛域名匹配并伪装 User-Agent

为多个子域名配置通配的代理节点，并加上特殊的请求头来绕过某些验证限制：

```json
[
  {
    "rules": "[store,cdn].steampowered.com",
    "target": "*.proxy.my-server.com",
    "keep-host": false,
    "headers": {
      "User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
      "X-Forwarded-For": "114.114.114.114"
    }
  }
]
```
