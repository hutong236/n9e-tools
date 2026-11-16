# n9e-tools — Nightingale v8 Excel 告警规则批量导入

Rust + Axum 独立 Web 工具，适配 **《夜莺告警配置表格（严重告警）_20261113_标红筛选.xlsx》** 的 11 Sheet 结构。本项目不是通用 Excel 解析器。

## 固定规则

- **不按颜色筛选**：读取「规则配置明细」中的全部有效规则（已上传模板共 **104 条**），保留 11 Sheet 数据关联；不要求 A 列标红。
- 按规则名称关联「规则配置明细」、「告警参数配置」、「严重告警清单」；检查其余 8 张 Sheet 存在，提示通知/采集前置条件。当前未将其余 8 张 Sheet 所有字段自动转换成告警配置。
- PromQL 采用「规则配置明细」H 列**原始表达式**；for 采用「告警参数配置」G 列**原始时长**。包含 `0m` 的合法规则不会被误判。
- 告警级别 S1；目标 Nightingale 业务组 ID 与数据源 ID 在界面填写，不自动按 Excel 中的 exporter 分组猜测。
- 同业务组**同名跳过**、只创建 `disabled=1` 规则、不自动绑定消息模板、接收组或通知路由。
- Excel → 全量解析 → 搜索 / 校验过滤 → 逐条勾选 / 禁用候选 / 删除候选 → 确认 → API 创建 → 逐条 JSON 结果。\n- 禁用候选和删除候选只作用于本次**浏览器端待导入列表**，不修改已存在的夜莺规则；导入创建依然默认 `disabled=1`。\n- 夜莺连接测试按钮通过实际 GET 业务组告警规则 API 检查 URL、Token、业务组访问权限；**暂不独立验证数据源 ID**。

## 快速开始

需要 Rust stable toolchain。

```bash
cargo test
cargo run --release
# 默认监听 http://127.0.0.1:8088
```

环境变量 `BIND=127.0.0.1:8088` 控制监听地址。默认只开放本机；建议仅通过 VPN 或可信跳板机使用，不能无认证直接暴露公网。

## Docker 启动

```bash
docker build -t n9e-tools:0.4 .
docker run --rm -p 127.0.0.1:8088:8088 n9e-tools:0.4
```

Docker 容器内监听 0.0.0.0:8088，推荐仅在宿主机 loopback 绑定，对远程操作使用受控代理或 VPN。项目的 Web 接口当前没有独立登录功能。

## HTTP 接口

**POST /api/preview**：multipart 表单中字段 `file` 为 XLSX，限制 16 MiB。返回 `preview_id`、全部有效规则及每条 error/warning。服务器仅在内存保留预览会话，最多保留约 20 个会话，重启后需要重新上传。

**POST /api/test-connection**：JSON 请求：\n\n```json\n{"url":"https://n9e.example.internal","token":"YOUR_TOKEN","group_id":1,"datasource_id":1}\n```\n\n会调用夜莺的业务组告警规则 API，检查 HTTP、JSON、业务错误并返回当前业务组规则数量。测试成功**不表示**数据源 ID 已验证，也不表示 PromQL 已通过指标查询。\n\n**POST /api/import**：JSON 请求：

```json
{
  "url": "https://n9e.example.internal",
  "token": "REPLACE_ME",
  "group_id": 1,
  "datasource_id": 1,
  "preview_id": "USE_ID_FROM_PREVIEW",
  "rows": [5, 6],
  "confirm": true
}
```

服务端会从缓存的原始 Excel 解析结果取回上述行，不接受客户端直接替换 PromQL/持续时间。查询已有规则失败会停止写入，以避免无法正确执行同名跳过。使用 `X-User-Token`，对接 `GET/POST /api/n9e/busi-group/:id/alert-rules`，设置 `datasource_queries` 限定实例。创建接口发送**单元素 JSON 数组**，并检查 `dat` 中对应规则名的错误字符串。顶层 `prom_ql` 为空，表达式放在 `rule_config.queries`。

## 当前验收状态及限制

- 已根据用户上传的 Excel 独立核对：11 张 Sheet、104 条规则，其中 54 条原先有红色填充（新版本不再据此筛选）；`0m` 持续时间保持不变。
- GitHub Actions 工作流包含 `cargo test --all-targets` 和 `cargo build --release`；请以仓库 Actions 运行状态判断 Rust 编译是否通过。
- **尚未连接用户的实际 Nightingale v8 环境**，请求体各字段须在该目标小版本上执行一条禁用规则的灰度验收。
- API 返回成功只表示请求被接受；当前没有逐条回读或强一致核对，也没有事务回滚。
- 官方当前路由的按业务组列表接口返回全部规则；对于实际部署的 v8 小版本仍需验证列表响应完整，保证同名跳过。
- 对请求 URL 未做出站 DNS/IP 白名单约束（SSRF 风险）；服务必须仅部署在可信的运维网段，并增加网关访问控制。
- 不自动迁移告警通知渠道、接收组、采集侧标签，不验证真实 PromQL 指标存在性。

## 测试

```bash
cargo test --all-targets
cargo build --release
```

预览第一步可不提供 Nightingale 连接信息；实际导入前请使用测试业务组进行 1 条禁用规则试运行。不要把原始 Excel 当作可以直接激活的生产告警配置。

## 手动触发 GitHub Actions 多平台 Docker 镜像构建

进入仓库的 [Actions → Build multi-platform Docker image](https://github.com/hutong236/n9e-tools/actions/workflows/docker-multiarch.yml)，点击 **Run workflow**，选择 `main` 分支并设置：

- `tag`：镜像版本标签，例如 `v0.4.0`（必填，默认 `v0.4.0`）。
- `push_latest`：是否同时覆盖推送 `latest`（默认关闭，发布正式稳定版时才建议勾选）。

工作流使用 Docker Buildx + QEMU，在 GitHub 托管的 Ubuntu Runner 上构建 **linux/amd64** 和 **linux/arm64**，并将统一的多架构 manifest 推送到 GitHub Container Registry：

```text
ghcr.io/hutong236/n9e-tools:v0.4.0
```

无需额外提供 GHCR 密码。工作流通过 `secrets.GITHUB_TOKEN` 登录，具备 `packages: write` 最小权限。首次发布的 GHCR Package 可见性可能默认为 Private；若需要匿名拉取，请到 GitHub 的 Package Settings 设置为 Public。

### 拉取和部署

```bash
docker pull ghcr.io/hutong236/n9e-tools:v0.4.0

docker run --rm -p 127.0.0.1:8088:8088 \
  ghcr.io/hutong236/n9e-tools:v0.4.0
```

Docker 会按宿主机架构自动选择 amd64 或 arm64 镜像。可以使用：

```bash
docker buildx imagetools inspect ghcr.io/hutong236/n9e-tools:v0.4.0
```

检查 manifest 中是否包含两个目标平台。

**安全提醒：** 此 Web 工具目前没有独立登录鉴权，不要将 8088 端口直接暴露公网；建议仅通过 VPN / SSH 隧道访问。工作流只在人工点击 **Run workflow** 时发布镜像；已有 Rust CI 仍按 push/PR 触发。


## V0.4.0 Web 页面与批量操作

- 全新简洁布局：Excel 上传与夜莺连接并排，规则区域突出关键统计；长校验说明不再逐行展开，点击“详情”查看原始 PromQL、建议表达式与完整警告。
- 规则支持关键词、校验状态、规则分组过滤与 20/50/100 条分页。切换搜索/分页会保留勾选状态。
- **批量禁用候选、批量删除候选仅作用于当前筛选结果中已勾选的规则**。隐藏的其他选中规则不受影响；操作前显示确认对话框。
- “恢复筛选禁用”恢复当前筛选条件可见的已禁用候选；“恢复删除”恢复本次预览中所有已删除条目。
- 这些操作只修改浏览器中的临时待导入清单，**绝不删除或启用夜莺已有规则**。新建告警规则仍是 `disabled=1`。
- 连接测试成功后方可点击导入；变更 URL/Token/业务组/数据源会要求重新测试。测试结果暂不确认数据源 ID 的有效性。
- 当前尚未在用户的 Nightingale 实例执行实际写入验收；建议先在测试业务组导入少量禁用规则。

## GitHub Actions：手工打包 Linux x86 和 Mac ARM 二进制程序

在 [Actions → Build downloadable binaries](https://github.com/hutong236/n9e-tools/actions/workflows/binary-release.yml) 中点击 **Run workflow**，选择 `main`，输入包名标签（默认 `v0.4.0`）。

此任务与 Docker 镜像工作流相互独立，不会自动推送镜像。两个平台均成功编译和通过测试后，工作流将**自动创建新的 GitHub Release**，并上传两个压缩包及对应的 SHA-256 校验文件。两台 GitHub Runner 分别原生编译：

| 平台 | Runner | Rust target | 结果 |
| --- | --- | --- | --- |
| Linux x86_64 | `ubuntu-22.04` | `x86_64-unknown-linux-gnu` | `n9e-tools-v0.4.0-linux-x86_64.tar.gz` |
| macOS Apple Silicon | `macos-15`（ARM64） | `aarch64-apple-darwin` | `n9e-tools-v0.4.0-macos-arm64.tar.gz` |

运行成功后，打开 [GitHub Releases](https://github.com/hutong236/n9e-tools/releases)，找到本次输入的版本号，直接下载对应平台的压缩包和 SHA-256 文件。构建过程也会把产物保留在 Actions Artifacts 中（30 天）。每个平台包含可执行程序 `n9e-tools` 和 README。构建会执行 Rust 测试，并通过 `file` 命令检查二进制文件。

Linux 安装示例：

```bash
tar -xzf n9e-tools-v0.4.0-linux-x86_64.tar.gz
cd n9e-tools-v0.4.0-linux-x86_64
chmod +x n9e-tools
./n9e-tools
```

macOS Apple Silicon 安装示例：

```bash
tar -xzf n9e-tools-v0.4.0-macos-arm64.tar.gz
cd n9e-tools-v0.4.0-macos-arm64
chmod +x n9e-tools
./n9e-tools
```

默认监听 `127.0.0.1:8088`。Linux 产物是 **glibc 动态链接**程序，不是 musl 静态可执行文件；在较老发行版运行前要核对 libc 兼容性。macOS 的 GitHub Actions 未签名二进制文件可能受到 Gatekeeper 限制，请仅对经过核实的构建产物按组织安全策略处理。当前 Web 服务没有独立的登录鉴权，不要直接对公网开放。

> 发布保护：只有两个平台都成功，才会执行校验与 Release 发布；如果相同版本号的 Release 已存在，流程会报错退出，不会覆盖旧版本。首次运行需确保仓库 Actions 允许 `GITHUB_TOKEN` 的 `contents: write` 权限。是否发布成功以 Release 页面出现新版本及完整附件为准。

### 自动发布 Release 流程

工作流位置：`.github/workflows/binary-release.yml`。手工点击 Run workflow，输入新版本号（例如 `v0.4.1`），按顺序执行：

1. Linux x86_64 与 macOS ARM64 **并行构建、运行测试**。
2. 上传每个平台的 `.tar.gz` 和 `.sha256` 到工作流 Artifacts。
3. 构建全部成功后，下载两个平台的产物，**验证 SHA-256**。
4. 检查版本号未发布，使用 GitHub CLI 创建对应 Git tag 和 GitHub Release。
5. 在该 Release 中上传两个二进制压缩包与两个校验文件。

Release 地址示例：`https://github.com/hutong236/n9e-tools/releases/tag/v0.4.1`。

注意：如果原版本号已创建 Release，请手工触发时输入新的版本号；不要重复发布相同标签。
