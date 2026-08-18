# Reactor 更新与兼容策略

状态：M10.5 稳定版契约（2026-08-19）

## 更新通道

- `stable` 是默认通道，只接收已经通过全仓测试、模拟器端到端门禁和发布健康检查的版本。
- `beta` 是显式选择的预览通道，不会静默把稳定版用户切换过去。
- macOS、Windows 和 Linux artifact 必须分别声明目标平台、架构、SHA-256、文件大小和下载 URL；平台或架构不匹配时拒绝安装。

正式 manifest 使用 schema v1，并由 CI 的独立发布密钥以 Ed25519 签名。桌面构建只包含公钥；私钥只能存在于发布 CI 的受保护 Secret 中，不进入源码、安装包、诊断包或本地运行数据。开发构建未注入 `REACTOR_UPDATE_PUBLIC_KEY` 时必须显示“未配置发布公钥”并拒绝未签名更新，不能降级到仅校验哈希。

签名载荷是 UTF-8 JSON，字段顺序固定为 `schemaVersion, channel, version, publishedAt, compatibility, artifacts, signatureAlgorithm, signatureKeyId`，不包含签名值本身。Reactor 核心会先验证 Ed25519，再检查数据库/Flow/Result 兼容矩阵以及每个 artifact 的 HTTPS、SHA-256 和大小；任一检查失败都不进入 staging。

## 分阶段安装与回滚

1. 在非测量阶段检查更新；任何任务运行时禁止替换二进制或受管工具。
2. 下载到版本隔离的 staging 目录，同时校验 manifest 签名、artifact SHA-256、平台、架构和最小兼容版本。
3. 保留当前可启动版本和数据库迁移前备份，再原子切换到候选版本。
4. 候选版本必须完成启动、数据库只读/迁移、Runner 握手、内置适配器清单和本地历史读取健康检查。
5. 健康检查失败时恢复上一版本及迁移前数据库；失败版本进入隔离区，不循环重试。
6. 更新成功后只删除更旧的 staging 文件，原始性能结果、报告、Flow 锁和受管工具缓存不随应用更新清除。

## 1.x 稳定版兼容承诺

- Reactor Flow v1、Result v1、Flow Lock v1、插件契约 v1 和数据库历史在整个 1.x 系列保持可读。
- 数据库升级只采用事务 migration；旧版遇到更高 schema 必须拒绝写入，不能覆盖未来版本数据。
- 新字段默认向后兼容并具有明确默认值；删除字段、改变指标定义、改变哈希规范或破坏 CLI 退出码属于主版本变化。
- 指标定义或采集器版本不兼容时拒绝同列比较，不通过迁移伪造可比性。
- 1.x 内允许增加 Flow 动作和诊断字段，但旧动作、既有锁哈希和已生成报告仍可验证。

## 发布门禁

稳定通道发布前必须通过：格式化、全仓测试、严格 clippy、前端生产构建、Android/iOS 对应模拟器门禁、数据库升级/回滚测试、诊断包隐私测试、签名 manifest 校验和候选版本健康检查。Apple notarization、Windows Authenticode 和对应平台真实安装验证仍需要各自开发者账号/环境，缺失时必须标记“未执行”，不能冒充通过。
