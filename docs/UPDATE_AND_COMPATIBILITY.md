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

桌面端的“检查并暂存更新”真实执行 HTTPS manifest/安装包下载；manifest 最大 1 MiB，安装包最大 1 GiB，解压后最大 2 GiB/10 万文件。下载按流写入磁盘，不把完整安装包常驻内存；最终重定向、平台/架构、声明大小、SHA-256 或安全解包任一失败都会在安装切换前终止。候选 App 被复制到当前安装同一卷后才允许重启切换，避免跨卷原子重命名失效。

安装由同一个已发布 Reactor 二进制的无界面 Helper 执行。Helper 等待桌面进程退出，保留旧 App 和 SQLite/WAL/SHM 快照，原子切换候选版本，并调用候选二进制的健康探针。健康探针必须同时证明版本号匹配、数据库与历史可读、Worker 命令入口可启动、受管 Maestro/ADB/Flashlight/Trace Processor 声明完整；失败时恢复旧 App 与数据库。最近事务的 staged/healthy/rolled_back/quarantined 状态会在设置页显示。

`.github/workflows/release.yml` 是唯一正式 manifest 生成入口：发布 CI 从隔离 Secret 注入原始 Ed25519 公钥和 PKCS#8 私钥，先验证二者匹配，再构建内含公钥的 App，最后由 `tools/sign_update_manifest.py` 对固定字段顺序载荷签名。Stable 使用 GitHub latest release；Beta 使用不影响 latest 的 prerelease artifact 和滚动的仅 manifest 通道。

## 1.x 稳定版兼容承诺

- Reactor Flow v1、Result v1、Flow Lock v1、插件契约 v1 和数据库历史在整个 1.x 系列保持可读。
- 数据库升级只采用事务 migration；旧版遇到更高 schema 必须拒绝写入，不能覆盖未来版本数据。
- 新字段默认向后兼容并具有明确默认值；删除字段、改变指标定义、改变哈希规范或破坏 CLI 退出码属于主版本变化。
- 指标定义或采集器版本不兼容时拒绝同列比较，不通过迁移伪造可比性。
- 1.x 内允许增加 Flow 动作和诊断字段，但旧动作、既有锁哈希和已生成报告仍可验证。

## 发布门禁

稳定通道发布前必须通过：格式化、全仓测试、严格 clippy、前端生产构建、Android/iOS 对应模拟器门禁、数据库升级/回滚测试、诊断包隐私测试、签名 manifest 校验和候选版本健康检查。Apple notarization、Windows Authenticode 和对应平台真实安装验证仍需要各自开发者账号/环境，缺失时必须标记“未执行”，不能冒充通过。
