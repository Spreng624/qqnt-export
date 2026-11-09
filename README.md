# QQNT Export

QQNT Export 是一个运行在 Windows 上的 QQNT 聊天记录命令行工具。它读取本机 QQ 数据库，在本地完成解密、消息解析、全文检索和导出，不连接 QQ 服务，也不会上传聊天内容。

主要能力：

- 自动发现 QQNT 账号和 `nt_msg.db`
- 支持群聊与私聊记录
- 解析文本、图片、文件、表情、引用、转发、通话和系统消息
- 建立本地 SQLite FTS5 索引，可搜索会话和聊天内容
- 导出 Markdown、HTML、JSON、JSONL 和 CSV
- `auth` 通过调试器断点截获数据库密钥，并用 Windows DPAPI 加密保存
- `open` 只使用已保存的密钥解密和索引，不再联网或重新取钥
- 所有聊天处理均在本地完成

QQ 更新可能改变数据库表结构、消息字段或密钥提取位置。遇到不兼容版本时，工具会保留无法识别的 Protobuf wire 字段，便于后续适配。

## 开发环境

需要：

- Windows 10/11 x64
- Node.js 24
- Rust stable
- Visual Studio C++ Build Tools

安装依赖并运行测试：

```powershell
npm ci
npm test
```

生成发布文件：

```powershell
npm run build
```

构建使用 Node.js 官方 SEA（Single Executable Applications）。JavaScript CLI、Rust 核心和 SQLCipher 运行文件会打包进一个程序，输出到：

```text
dist/qqnt-export.exe
dist/SHA256SUMS.txt
dist/BUILD_INFO.txt
```

SQLCipher 使用固定版本并在构建时校验 SHA-256。推送 `v*` 标签会触发 `.github/workflows/release.yml`，自动构建并发布 GitHub Release。

目录结构：

```text
bin/       CLI 入口
src/       Node.js 命令行与运行时
core/      Rust 核心
scripts/   构建脚本
test/      测试
```

## 使用方式

从 GitHub Release 下载 `qqnt-export.exe` 后，可直接运行：

```powershell
.\qqnt-export.exe -h
.\qqnt-export.exe --help
.\qqnt-export.exe doctor
```

第一步，获取并保存账号密钥。运行后会提示你自行完全退出 QQ：

```powershell
.\qqnt-export.exe auth --account 123456789
```

检测到 QQ 正在运行时，程序会等待你手动完全退出，不会主动结束 QQ 进程。退出后会自动启动调试模式 QQ，请在弹出的窗口中登录或确认目标账号。密钥截获成功后会先用 SQLCipher 验证，再通过 Windows DPAPI 加密保存。Key 文件位于 `%LOCALAPPDATA%\qqnt-export\keys`，不会写入配置或日志。

第二步，建立本地索引：

```powershell
.\qqnt-export.exe open --account 123456789
```

`open` 只读取已保存的密钥，不会再次启动或调试 QQ。QQ 数据变化后，重复执行 `open` 即可重建索引；如果密钥失效或 QQ 重建了数据库，重新执行 `auth`。

读取和导出记录：

```powershell
.\qqnt-export.exe chats
.\qqnt-export.exe history group:123456789
.\qqnt-export.exe history c2c:987654321 --limit 100
.\qqnt-export.exe search "作业" --chat group:123456789
.\qqnt-export.exe export group:123456789 -f markdown -o chat.md
.\qqnt-export.exe export group:123456789 -f html -o chat.html
```

常用选项：

```text
-h, --help          显示帮助
-V, --version       显示版本
-a, --account       选择 QQ 账号
-n, --limit         限制返回数量
-j, --json          输出 JSON
-f, --format        导出格式
-o, --output        输出文件
-y, --yes           跳过交互确认
```

聊天记录索引默认保存在：

```text
%LOCALAPPDATA%\qqnt-export
```

删除本地索引和运行时缓存：

```powershell
.\qqnt-export.exe clean
```

请只处理本人拥有或已获授权的 QQ 数据，并遵守 QQ 软件许可协议和当地法律。
