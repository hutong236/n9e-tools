# n9e-tools — Nightingale v8 Excel 红色告警规则导入

Rust + Axum 独立 Web 工具，适配 **《夜莺告警配置表格（严重告警）_20261113_标红筛选.xlsx》** 的 11 Sheet 结构。本项目不是通用 Excel 解析器。

## 固定规则

- 识别「规则配置明细」A 列**红色单元格背景填充**，目标 **54 条**（总计 104 条）；样式由 XLSX 的 `styles.xml` 动态识别，不硬编码 style 12。
- 按规则名称关联「规则配置明细」、「告警参数配置」、「严重告警清单」；检查其余 8 张 Sheet 存在，提示通知/采集前置条件。当前未将其余 8 张 Sheet 所有字段自动转换成告警配置。
- PromQL 采用「规则配置明细」H 列**原始表达式**；for 采用「告警参数配置」G 列**原始时长**。包含 `0m` 的合法规则不会被误判。
- 告警级别 S1；目标 Nightingale 业务组 ID 与数据源 ID 在界面填写，不自动按 Excel 中的 exporter 分组猜测。
- 同业务组**同名跳过**、只创建 `disabled=1` 规则、不自动绑定消息模板、接收组或通知路由。
- Excel → 校验 → 预览 → 勾选 → 确认 → API 创建 → 逐条 JSON 结果。

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
docker build -t n9e-tools:0.2 .
docker run --rm -p 127.0.0.1:8088:8088 n9e-tools:0.2
```

Docker 容器内监听 0.0.0.0:8088，推荐仅在宿主机 loopback 绑定，对远程操作使用受控代理或 VPN。项目的 Web 接口当前没有独立登录功能。

## HTTP 接口

**POST /api/preview**：multipart 表单中字段 `file` 为 XLSX，限制 16 MiB。返回 `preview_id`、54 条规则及每条 error/warning。服务器仅在内存保留预览会话，最多保留约 20 个会话，重启后需要重新上传。

**POST /api/import**：JSON 请求：

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

- 已根据用户上传的 Excel 独立核对：11 张 Sheet、104 条明细、54 条红色填充；标红集合中有 **11 条原始持续时间为 0m**。
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
