# n9e-tools — Nightingale v8 Excel 告警规则批量导入

Rust + Axum 独立 Web 工具，适配 **《夜莺告警配置表格（严重告警）_20261113_标红筛选.xlsx》** 的 11 Sheet 结构。本项目不是通用 Excel 解析器。

## 固定规则

- **不按颜色筛选**：读取「规则配置明细」中的全部有效规则（已上传模板共 **104 条**），保留 11 Sheet 数据关联；不要求 A 列标红。
- 按规则名称关联「规则配置明细」、「告警参数配置」、「严重告警清单」；检查其余 8 张 Sheet 存在，提示通知/采集前置条件。当前未将其余 8 张 Sheet 所有字段自动转换成告警配置。
- PromQL 采用「规则配置明细」H 列**原始表达式**；for 采用「告警参数配置」G 列**原始时长**。包含 `0m` 的合法规则不会被误判。
- 告警级别 S1；用户只需填写 Nightingale URL 和 X-User-Token，点击「测试连接并选择」打开弹窗，选择业务组和数据源后自动回填 ID。不会按 Excel 中的 exporter 分组猜测。
- 同业务组**同名跳过**、只创建 `disabled=1` 规则、不自动绑定消息模板、接收组或通知路由。
- Excel → 全量解析 → 搜索 / 校验过滤 → 逐条勾选 / 禁用候选 / 删除候选 → 确认 → API 创建 → 逐条 JSON 结果。\n- 禁用候选和删除候选只作用于本次**浏览器端待导入列表**，不修改已存在的夜莺规则；导入创建依然默认 `disabled=1`。\n- 夜莺连接测试通过 API 读取可访问的业务组和数据源，用户在弹窗中选定后才能执行导入。

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
docker build -t n9e-tools:0.7 .
docker run --rm -p 127.0.0.1:8088:8088 n9e-tools:0.7
```

Docker 容器内监听 0.0.0.0:8088，推荐仅在宿主机 loopback 绑定，对远程操作使用受控代理或 VPN。项目的 Web 接口当前没有独立登录功能。

## HTTP 接口

**POST /api/preview**：multipart 表单中字段 `file` 为 XLSX，限制 16 MiB。返回 `preview_id`、全部有效规则及每条 error/warning。服务器仅在内存保留预览会话，最多保留约 20 个会话，重启后需要重新上传。

**POST /api/test-connection**：JSON 请求：\n\n```json\n{"url":"https://n9e.example.internal","token":"YOUR_TOKEN","group_id":1,"datasource_id":1}\n```\n\n会调用夜莺的业务组告警规则 API，检查 HTTP、JSON、业务错误并返回当前业务组规则数量。测试成功**不表示**数据源 ID 已验证，也不表示 PromQL 已通过指标查询。\n\n**POST /api/import**：JSON 请求（兼容旧客户端）：

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

服务端会从缓存的原始 Excel 解析结果取回上述行，不接受客户端直接替换 PromQL/持续时间。查询已有规则失败会停止写入，以避免无法正确执行同名跳过。使用 `X-User-Token`，通过 `GET /api/n9e/busi-group/:id/alert-rules` 查询重复名称，通过 `POST /api/n9e/busi-group/:id/alert-rules/import` 创建（Nightingale v8 新版导入接口），设置 `datasource_queries` 限定实例。创建请求明确发送 `notify_version: 1` 和空的 `notify_rule_ids: []`，使新建规则可在 Nightingale 中关联新版通知规则；**默认不会自动关联任何通知规则，规则仍以 `disabled=1` 创建**。创建接口发送**单元素 JSON 数组**，并检查 `dat` 中对应规则名的错误字符串。顶层 `prom_ql` 为空，表达式放在 `rule_config.queries`。

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

- `tag`：镜像版本标签，例如 `v0.7.0`（必填，默认 `v0.7.0`）。
- `push_latest`：是否同时覆盖推送 `latest`（默认关闭，发布正式稳定版时才建议勾选）。

工作流使用 Docker Buildx + QEMU，在 GitHub 托管的 Ubuntu Runner 上构建 **linux/amd64** 和 **linux/arm64**，并将统一的多架构 manifest 推送到 GitHub Container Registry：

```text
ghcr.io/hutong236/n9e-tools:v0.7.0
```

无需额外提供 GHCR 密码。工作流通过 `secrets.GITHUB_TOKEN` 登录，具备 `packages: write` 最小权限。首次发布的 GHCR Package 可见性可能默认为 Private；若需要匿名拉取，请到 GitHub 的 Package Settings 设置为 Public。

### 拉取和部署

```bash
docker pull ghcr.io/hutong236/n9e-tools:v0.7.0

docker run --rm -p 127.0.0.1:8088:8088 \
  ghcr.io/hutong236/n9e-tools:v0.7.0
```

Docker 会按宿主机架构自动选择 amd64 或 arm64 镜像。可以使用：

```bash
docker buildx imagetools inspect ghcr.io/hutong236/n9e-tools:v0.7.0
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

## GitHub Actions：手工打包 Linux、macOS 与 Windows 二进制程序

在 [Actions → Build downloadable binaries](https://github.com/hutong236/n9e-tools/actions/workflows/binary-release.yml) 中点击 **Run workflow**，选择 `main`，输入包名标签（默认 `v0.7.0`）。

此任务与 Docker 镜像工作流相互独立，不会自动推送镜像。三个平台均成功编译和通过测试后，工作流将**自动创建新的 GitHub Release**，并上传三个压缩包及对应的 SHA-256 校验文件。三台 GitHub Runner 分别原生编译：

| 平台 | Runner | Rust target | 结果 |
| --- | --- | --- | --- |
| Linux x86_64 | `ubuntu-22.04` | `x86_64-unknown-linux-gnu` | `n9e-tools-v0.7.0-linux-x86_64.tar.gz` |
| macOS Apple Silicon | `macos-15`（ARM64） | `aarch64-apple-darwin` | `n9e-tools-v0.7.0-macos-arm64.tar.gz` |
| Windows x86_64（64 位） | `windows-2022` | `x86_64-pc-windows-msvc` | `n9e-tools-v0.7.0-windows-x86_64.zip` |

运行成功后，打开 [GitHub Releases](https://github.com/hutong236/n9e-tools/releases)，找到本次输入的版本号，直接下载对应平台的压缩包和 SHA-256 文件。构建过程也会把产物保留在 Actions Artifacts 中（30 天）。每个平台包含可执行程序（Windows 为 `n9e-tools.exe`）和 README。构建会执行 Rust 测试，并通过 `file` 命令检查二进制文件。

Linux 安装示例：

```bash
tar -xzf n9e-tools-v0.7.0-linux-x86_64.tar.gz
cd n9e-tools-v0.7.0-linux-x86_64
chmod +x n9e-tools
./n9e-tools
```

macOS Apple Silicon 安装示例：

```bash
tar -xzf n9e-tools-v0.7.0-macos-arm64.tar.gz
cd n9e-tools-v0.7.0-macos-arm64
chmod +x n9e-tools
./n9e-tools
```

默认监听 `127.0.0.1:8088`。Linux 产物是 **glibc 动态链接**程序，不是 musl 静态可执行文件；在较老发行版运行前要核对 libc 兼容性。macOS 的 GitHub Actions 未签名二进制文件可能受到 Gatekeeper 限制，请仅对经过核实的构建产物按组织安全策略处理。当前 Web 服务没有独立的登录鉴权，不要直接对公网开放。

> 发布保护：只有两个平台都成功，才会执行校验与 Release 发布；如果相同版本号的 Release 已存在，流程会报错退出，不会覆盖旧版本。首次运行需确保仓库 Actions 允许 `GITHUB_TOKEN` 的 `contents: write` 权限。是否发布成功以 Release 页面出现新版本及完整附件为准。

### 自动发布 Release 流程

工作流位置：`.github/workflows/binary-release.yml`。手工点击 Run workflow，输入新版本号（例如 `v0.4.1`），按顺序执行：

1. Linux x86_64、macOS ARM64、Windows x86_64 **并行构建、运行测试**。
2. 上传 Unix 平台的 `.tar.gz`、Windows 的 `.zip` 及各自 `.sha256` 到工作流 Artifacts。
3. 三个平台构建全部成功后，下载三个平台的产物，**验证 SHA-256**。
4. 检查版本号未发布，使用 GitHub CLI 创建对应 Git tag 和 GitHub Release。
5. 在该 Release 中上传三个二进制压缩包与三个校验文件。

Release 地址示例：`https://github.com/hutong236/n9e-tools/releases/tag/v0.4.1`。

注意：如果原版本号已创建 Release，请手工触发时输入新的版本号；不要重复发布相同标签。

## V0.5.0：夜莺资源自动发现与回填

连接步骤：输入 **Nightingale 地址**、**X-User-Token**，点击「测试连接并选择」，工具会调用 `GET /api/n9e/busi-groups` 和 `POST /api/n9e/datasource/list`（请求体 `{"name":"","plugin_type":"","category":""}`），返回当前 Token 可访问的业务组和数据源列表。弹窗允许搜索、选择业务组和数据源；确认后将其真实 ID 自动写入只读字段，避免人为抄错。

服务端只返回资源 ID、名称及数据源类型，不会向浏览器转发 Nightingale 返回的敏感数据源连接字段。连接失败时按业务组请求、数据源请求分别提示错误。仅测试接口成功并不证明具备创建告警规则的权限；需要实际执行一次禁用规则导入才能确认写权限。

若取消弹窗，两个 ID 不会回填，不能执行导入。修改 URL 或 Token 后会清除之前的目标选择，必须重新测试。

**API：** `POST /api/discover`，body 为 `{"url":"https://n9e.example.internal","token":"YOUR_TOKEN"}`。响应格式：`{"groups":[{"id":1,"name":"业务组","kind":""}],"datasources":[{"id":2,"name":"VictoriaMetrics","kind":"prometheus"}]}`。

### Windows x86_64 运行方法

从对应 [GitHub Release](https://github.com/hutong236/n9e-tools/releases) 下载 `n9e-tools-v0.7.0-windows-x86_64.zip`，解压后在 PowerShell 中执行：

```powershell
cd .\\n9e-tools-v0.7.0-windows-x86_64
.\\n9e-tools.exe
```

在 Windows 浏览器访问 `http://127.0.0.1:8088`。如果 Windows Defender 防火墙弹出提示，应根据实际内网使用场景配置访问范围。Windows 发布包采用 MSVC 64 位目标（不是 32 位 x86）。

## V0.6.0：实时导入进度与结构化结果

导入时不再等待全部告警规则执行后才显示一段平铺文本。Web 页面使用 `POST /api/import-stream` 读取 **NDJSON 流**，每条规则的 Nightingale API 请求执行完毕后立即更新界面：

- **进度：** 当前执行规则、已处理/总数和百分比。百分比根据服务端已完成的规则数量计算，不是估算值。
- **统计：** 创建成功、同名跳过、失败/校验错误、剩余待处理四项指标。
- **结果明细：** 表格显示 Excel 行号、规则名称、执行状态及返回说明；支持状态过滤和关键字搜索。
- **导出：** 所有结果可以下载完整 JSON 或 UTF-8 BOM CSV（方便 Excel 打开），导出的是全部已处理记录，不受屏幕过滤或最多显示 200 条的限制。
- **异常：** 网络连接中断或服务端返回业务错误时，明确标为“已中断”，保留已接收到的部分结果；不将不确定状态伪装为成功。

NDJSON 事件示例：

```json
{"type":"started","total":2}
{"type":"current","index":1,"row":5,"name":"CPU 使用率"}
{"type":"item","index":1,"total":2,"item":{"row":5,"name":"CPU 使用率","status":"created","message":"API已接受创建请求（默认禁用）"}}
{"type":"current","index":2,"row":6,"name":"已有规则"}
{"type":"item","index":2,"total":2,"item":{"row":6,"name":"已有规则","status":"skipped","message":"同业务组内已存在同名规则"}}
{"type":"complete","total":2,"created":1,"skipped":1,"failed":0}
```

协议在响应头中使用 `Content-Type: application/x-ndjson`，逐行输出。该接口与原有 `POST /api/import` 复用同一套校验、同名跳过和默认禁用的创建逻辑，原接口保持可用。

**重要：** 流式页面断开连接不等于已取消服务端任务。服务端可能仍在创建规则，因此页面刷新、浏览器断网或请求异常后，必须先到 Nightingale 核对实际写入情况，不要直接重复点击导入。API 创建成功也不等于规则已被激活（仍默认禁用）。

如部署时经过 Nginx/反向代理，请关闭该接口的响应缓冲，以便逐条进度及时显示。服务端已设置 `X-Accel-Buffering: no`。

## V0.7.0：规则参数详情支持 J、K、L、M 列

已核对原始 Excel 的「告警参数配置」Sheet：J=留观时长、K=重复通知间隔、L=最大重复次数、M=采集侧必备标签。根据「规则配置明细」B 列规则名称匹配「告警参数配置」B 列；匹配不成功或 J/K/L 无法解析会显示校验错误。

- **J（留观时长）**：自动解析分钟到秒，导入 `recover_duration`；页面可以修改秒数。
- **K（重复通知间隔）**：自动解析到分钟，导入 `notify_repeat_step`；0 代表不重复提醒。
- **L（最大重复次数）**：支持「0（不限）」等原始格式，导入 `notify_max_number`；0 代表不限制。
- **M（采集侧必备标签）**：在规则详情中展示并允许编辑、检查、确认。这些标签必须在 exporter、Prometheus 指标或采集侧生效；M 的原文可能包含 `{{主机名}}` 等模板，不能被直接当成静态 `append_tags` 写入夜莺。页面会在有未确认标签时于导入前警告。
- **静态附加标签**：规则详情新增单独字段，每行一个实际静态 `key=value`（例如 `team=ops`），会导入 `append_tags`。不支持动态占位符，客户端与服务端双重校验。

页面保存后参数仅在本次预览会话中生效；服务端通过 `POST /api/import-stream` 或 `POST /api/import` 的可选 `parameters` 字段应用覆盖值；原始 Excel 源数据不会被修改。参数根据 Excel 行号映射，服务端从已上传工作簿的缓存中匹配规则，不允许客户端提交任意规则内容。后端在创建之前校验覆盖值范围：留观时长 0–86400 秒、重复间隔 0–10080 分钟、最大次数 0–10000、静态标签最多 32 个。

**接口示例：**

```json
{
  "preview_id": "已缓存的预览ID",
  "rows": [5],
  "parameters": {
    "5": {
      "recover_duration": 300,
      "notify_repeat_step": 60,
      "notify_max_number": 0,
      "label_requirements": "env=prod；host={{主机名}}",
      "append_tags": ["team=ops"]
    }
  }
}
```

上例只展示新增字段，实际调用仍需 `url`、`token`、`group_id`、`datasource_id`、`confirm` 等原有必填字段。标签确认只用于前置条件核对，不代表工具已验证实际指标具有该标签。

## 静态附加标签排查与回读验证

在「规则详情 → 规则静态附加标签」输入框中**实际输入** `team=ops` 等 `key=value`，然后点击「保存当前规则参数」。输入框灰色的 `placeholder` 仅是提示，**不算配置**。规则列表会显示该规则已配置的标签数量，导入确认窗口会展示本次选中规则中实际保存静态标签的规则数量及示例；如显示 `0 / N` 则没有任何规则配置静态标签。

**同名跳过：** 本工具不会更新已有同名规则，重复导入不能添加/修改附加标签。需要在 Nightingale 中编辑已有规则，或使用新的规则名称在测试业务组验证。

**创建回读：** 新建规则带有静态标签时，服务端会再次 GET 业务组告警规则列表，读取对应规则的 `append_tags`，结果分三类：

- `verified`：已回读且包含全部请求的标签；
- `mismatch`：已找到规则及其标签字段，但未包含全部请求的标签；
- `unverified`：规则尚未出现在列表中、列表没有返回标签字段，或回读失败。

上述状态会记录在下载的结果 JSON（`tags_requested`、`tags_verification`）中，并在导入结果明细显示。创建 API 接受不代表标签一定持久化；如果为 `mismatch` 或 `unverified`，请进入 Nightingale 规则编辑页确认。列表回读可能存在短暂同步延迟；当前不会自动覆盖既有规则。

## 单条规则强制覆盖（已存在同名规则）

规则预览表「操作」列新增**覆盖**按钮。批量导入仍然实行**同业务组同名跳过**，不会由于新增覆盖功能而覆盖整批规则。

一条规则的强制覆盖必须依次完成：

1. 选择 Nightingale URL、Token、业务组、数据源，并先预览 Excel。点击该规则所在行的「覆盖」按钮。
2. 工具根据 **Excel 行号和规则名称** 读取目标业务组的已有规则列表，要求恰有一条同名规则，然后通过 `GET /api/n9e/alert-rule/:id/pure` 读取该规则的完整配置。弹窗列出原值与覆盖后值，并要求勾选“我已检查目标规则 ID 和配置差异”。
3. 在弹窗中完整输入目标规则名称（区分大小写），再点击“进入最终确认”。
4. 最后点击“确认覆盖这一条规则”。服务端要求五分钟内有效、只能使用一次的确认凭据，并再次回读目标规则；若内容与预览不一致，直接拒绝，避免覆盖其他人刚刚修改过的配置。
5. 通过 Nightingale v8 的 `PUT /api/n9e/busi-group/:id/alert-rule/:arid` **更新已有规则而不是删除重建**；完成后再次读取该规则，提示关键字段是否与目标配置一致。

本工具明确覆盖：原始 PromQL（仅单条 Prometheus 查询）、告警持续时间、评估间隔、数据源、备注、摘要/描述/建议、J/K/L 恢复和重复通知参数，以及当前单条规则保存的静态 `append_tags`。未配置静态附加标签时，**覆盖会清空该规则现有的静态附加标签**，弹窗会列出这项差异。保留目标规则的启用/禁用状态、通知规则关联、回调、生效时段和其他 Excel 不管理的 Nightingale 属性。

**严格限制：** 只支持已有的单查询 Prometheus 告警规则；不支持批量覆盖、多查询或 Prometheus v2 规则。源规则校验有错误时禁止覆盖，找不到同名规则或同名规则不唯一也禁止覆盖。每次请求只能覆盖一个目标规则，不支持删除后重建，覆盖操作无法在工具中自动撤销；操作前建议在 Nightingale 备份原规则。执行要求 Nightingale Token 具备规则读取权限、业务组写权限和 `/alert-rules/put` 权限。CI 使用模拟 Nightingale 验证，不代表已在生产环境执行覆盖。

### 强制覆盖前置条件：目标规则必须 Disabled

**仅允许覆盖 Nightingale 中已停用（`disabled=1`）的现有规则。** 目标规则处于启用（`disabled=0`）、接口缺少 `disabled` 字段、或者状态不是数字 `1` 时，一律拒绝操作；工具不会自动停用规则。必须先在 Nightingale Web 中手动停用目标规则，再重新进行覆盖确认。

服务端同时在 **准备覆盖（prepare）** 和 **最终写入（execute）** 两个阶段检查目标状态。若第一次检查后规则被重新启用，最终提交会返回 HTTP 409 且不调用更新接口。写入后回读也会核验 `disabled=1`。即使满足停用条件，仍必须通过逐条配置差异审阅、风险勾选、输入完整规则名称、最终确认和一次性凭据校验。

请注意：这项限制只应用于**强制覆盖已有规则**。普通批量新建仍默认创建为 Disabled，同名规则仍跳过。


### Excel「级别 / 严重程度」筛选

规则列表现在支持按 Excel 的 **级别** 与 **严重程度** 分别筛选，并可与规则分组、校验状态及快速搜索叠加使用。筛选选项从上传的工作簿中实际存在的非空值动态生成，重置筛选会同时清空两个条件。数据来自「规则配置明细」表对应标题列；如果缺失，尝试按规则名称读取「严重告警清单」中的同名标题列。不会根据其他含义的列猜测级别。

这两个字段属于 **Excel 分类元数据**：可在预览和规则详情中查看，用于选择候选规则，**不会修改 Nightingale 新建规则所用的严重级别（目前固定 S1）**，也不会参与导入配置校验。若上传的模板中找不到对应标题，筛选框仅显示“全部级别 / 全部严重程度”，不会虚构选项。

### Nightingale v8 新版通知规则兼容性

- 新建告警规则使用 `POST /api/n9e/busi-group/:id/alert-rules/import`，不是旧的 `POST /api/n9e/busi-group/:id/alert-rules`。这与 Nightingale v8 服务端导入流程保持一致。
- 请求显式传递 `notify_version: 1`、`notify_rule_ids: []`。用户仍须到 Nightingale 告警规则详情中**选择并保存需要关联的通知规则**，最后按审核流程启用告警；不选通知规则则不会凭空发送通知。
- **此修复只针对新创建的告警规则。** 本工具继续遵循“同名跳过”，不会擅自升级之前以 `notify_version: 0` 创建的告警规则。旧规则需在 Nightingale UI 中手动切换到新版通知并关联规则，或经过另行审核的迁移操作处理。
- 单条强制覆盖仍保留原告警规则的通知设置，不会把已有规则的旧通知模式悄悄改为新版，避免意外更改现有通知行为。


### 下载标准 Excel 模板

打开工具页面，在「上传 Excel」区域点击 **下载 Excel 模板**，即可通过 `GET /api/template` 下载 `n9e-alert-rules-template.xlsx`，无需配置 Nightingale URL 或 Token。模板由程序内置的 11 Sheet 结构生成，随各平台独立二进制发布，不依赖外部 Excel 文件。

模板为空白业务数据，填写示例放在「说明」工作表内，不会被误识别为待导入告警。保留工作表名称和列顺序，自第 5 行录入：

- 「规则配置明细」：B 列告警名称、H 列原始 PromQL；F/G 列分别为严重程度/级别筛选元数据。
- 「告警参数配置」：B 列告警名称与明细表完全一致；G 列原始持续时间、J 列留观时长、K 列重复通知间隔、L 列最大重复次数。
- 「严重告警清单」：可选补充 Exporter、分类及备用持续时间；其余工作表可留空，但不要删除。
- 新建规则仍默认 Disabled，使用新版通知模式；通知规则由用户在 Nightingale 中手动关联。

模板结构以当前解析器为准，不含原工作簿的生产规则、公式、业务说明和专用样式。原有 11 Sheet Excel 上传方式保持不变。
