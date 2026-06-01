# ANOLISA Runnable Framework Contract

本文定义 `anolisa` CLI 的第一阶段可运行框架规范。它不是替代
`anolisa-cli-design.md`，而是把老板设计文档中的方向落成可分工、可验收的工程契约。

目标是让不同开发同学可以并行实现 CLI、Manifest、环境探测、Capability Resolver、
Adapter、安装执行和 AgentSight P0 接入，而不互相阻塞。

## 1. 当前结论

`anolisa` 是 ANOLISA 解决方案的本地管家入口，负责安装、配置、健康检查和状态追踪。
`cosh` 是后续数据面形态，当前不作为主入口。

第一阶段要先做出一个“能跑起来的框架”，含义不是所有组件都能生产级安装，而是这条最小链路可以闭环：

```text
env probe -> manifest load -> capability list -> enable dry-run plan
          -> runtime/adapter/osbase direct surface -> status/doctor skeleton
```

P0 是 `agent-observability` / AgentSight。P1 是 Tokenless、Sec-Core、ws-ckpt 的 runtime 和 adapter 能力。`build-all.sh` 只作为 develop/source-build 场景保留，不再作为客户安装入口；现有 OpenClaw/Hermes 脚本只能作为兼容参考，不作为 adapter 产品主路径。

## 2. 什么叫能跑起来

第一阶段验收标准如下。

| 编号 | 能力 | 验收命令 | 必须满足 |
|---|---|---|---|
| R1 | CLI 上下文生效 | `anolisa --json list` | global flags 能传入 handler，JSON 输出有效 |
| R2 | 真实环境探测 | `anolisa env --json` | 输出真实 kernel/distro/arch/platform/frameworks，不再使用 placeholder |
| R3 | Manifest catalog 可加载 | `anolisa list --json` | 能加载 capability + runtime + osbase manifest，返回 9 个 capability |
| R4 | 可用性判断 | `anolisa list` | 每个 capability 有 `available/degraded/unavailable` 状态和原因 |
| R5 | AgentSight P0 计划 | `anolisa enable agent-observability --dry-run --json` | 返回安装计划、权限要求、降级/不可用原因，不修改系统 |
| R6 | Runtime 直达入口 | `anolisa runtime list --available --json` | 返回 runtime component catalog，不写死表格 |
| R7 | Adapter 探测入口 | `anolisa adapter scan --json` | 能探测 `cosh/openclaw/hermes/mcp`，只读不安装 |
| R8 | TargetProvider 可接入 | `anolisa adapter install tokenless openclaw --dry-run` | 能基于 target provider 生成 binding plan；旧 adapter runner 仅作为兼容后端 |
| R9 | 状态文件可读写 | `anolisa status --json` | 能读取 installed state；未安装时返回空状态而非占位文本 |
| R10 | Schema 测试 | `cargo test --workspace` | capability/component manifest schema 和关键 JSON 输出有测试 |
| R11 | 预编译分发优先 | `anolisa enable agent-observability --dry-run --json` | 默认计划使用预编译组件产物；只有显式 `--from-source` 才走 build backend |

不要求第一阶段完成真实安装所有组件；但必须能给出可信 dry-run plan，且 plan 的输入输出格式稳定。

## 3. 分层边界

`anolisa` 的模块分层如下。

```text
anolisa-cli
  - 解析 CLI 参数
  - 构造 CliContext
  - 调用 service
  - 只负责格式化输出，不做业务判断

anolisa-core
  - Manifest catalog
  - Capability Resolver
  - Dependency planner
  - Installed state
  - Transaction model

anolisa-env
  - EnvFacts
  - Probe runner
  - Framework scanner
  - Requirement gate

anolisa-platform
  - FsLayout
  - package manager
  - privilege
  - systemd/user service
  - Linux capability setcap

anolisa-build
  - cargo/make/npm/static build backend
  - develop/source-build backend
  - legacy build-all backend, only for development compatibility
```

Layer discipline:

- Tier 1 command 只暴露 capability 语义，例如 `agent-observability`，不要求用户知道 `agentsight`。
- Tier 2 command 可以暴露 component/target 语义，例如 `runtime install agentsight`、`adapter install tokenless openclaw`。
- Tier 1 必须经过 Capability Resolver；Tier 2 可以直接进入 runtime/adapter/osbase service。
- 所有写系统的动作必须先能生成 dry-run plan。

## 4. CLI 上下文契约

所有 handler 必须接收统一上下文，不能只接收 subcommand args。

```rust
pub struct CliContext {
    pub install_mode: InstallMode,
    pub prefix: Option<PathBuf>,
    pub output: OutputMode,
    pub dry_run: bool,
    pub verbosity: Verbosity,
    pub color: ColorMode,
    pub layout: FsLayout,
}

pub enum InstallMode {
    User,
    System,
}

pub enum OutputMode {
    Human,
    Json,
}
```

CLI dispatch 约束：

```rust
fn dispatch(ctx: CliContext, command: Commands) -> anyhow::Result<ExitCode>;
```

输出约束：

- `--json` 时 stdout 只能输出 JSON，诊断日志进 stderr。
- `--dry-run` 不允许修改文件、服务、package、state。
- human 输出可以有表格，但字段必须来自同一个 response model。
- 错误也要有 JSON 形态，至少包含 `code/reason/advice`.

错误 JSON 示例：

```json
{
  "ok": false,
  "error": {
    "code": "ENV_NOT_SATISFIED",
    "object": "agent-observability",
    "reason": "CAP_BPF is not available in the current container",
    "advice": "Enable AgentSight on the host, or rerun in a privileged container with CAP_BPF."
  }
}
```

## 5. Service 接口契约

### 5.1 Catalog service

Catalog service 负责加载内置 manifest。

```rust
pub struct Catalog {
    pub capabilities: HashMap<String, CapabilityManifest>,
    pub components: HashMap<String, ComponentManifest>,
}

pub struct CatalogLoadOptions {
    pub manifest_root: PathBuf,
    pub strict: bool,
}

pub trait CatalogService {
    fn load_builtin(&self, options: CatalogLoadOptions) -> Result<Catalog, CatalogError>;
    fn lint(&self, catalog: &Catalog) -> Vec<CatalogLint>;
}
```

要求：

- capability manifest 和 component manifest 分开加载。
- `strict=true` 时 schema 错误直接失败。
- `lint` 必须检查 capability 引用的 component 是否存在。
- `lint` 必须检查 placeholder 是否合法。
- `lint` 必须检查 manifest version 和 `anolisa_min_version`.

### 5.2 Environment service

```rust
pub struct ProbeOptions {
    pub use_cache: bool,
    pub refresh: bool,
    pub include_slow: bool,
}

pub trait EnvService {
    fn detect(&self, options: ProbeOptions) -> Result<EnvFacts, EnvError>;
    fn evaluate(&self, req: &RequirementExpr, facts: &EnvFacts) -> GateResult;
}
```

`EnvFacts` 最低字段：

```json
{
  "platform": "physical|vm|container",
  "container_runtime": "runc|gvisor|kata|firecracker|null",
  "hypervisor": "kvm|xen|hyperv|vmware|other|null",
  "distro": {"id": "alinux", "version": "4", "pkg_base": "rpm"},
  "kernel": {
    "version": "6.6.30",
    "btf_available": true,
    "cgroups_v2": true,
    "landlock_abi": 4,
    "kvm_available": true
  },
  "arch": "x86_64|aarch64|riscv64|other",
  "linux_capabilities": ["CAP_BPF", "CAP_PERFMON"],
  "filesystem": {"btrfs_available": true, "overlayfs_available": true},
  "frameworks": [
    {"name": "cosh", "kind": "first-party", "version": null, "location": null}
  ]
}
```

### 5.3 Capability service

```rust
pub struct ListRequest {
    pub filter: CapabilityFilter,
}

pub struct EnableRequest {
    pub capabilities: Vec<String>,
    pub feature: Option<String>,
    pub adapter: AdapterSelection,
    pub source: InstallSource,
}

pub enum AdapterSelection {
    FirstPartyOnly,
    AutoDetected,
    Explicit(Vec<String>),
}

pub enum InstallSource {
    Auto,
    Prebuilt,
    Source,
}

pub trait CapabilityService {
    fn list(&self, req: ListRequest, facts: &EnvFacts, catalog: &Catalog) -> Result<Vec<CapabilityView>, CapabilityError>;
    fn plan_enable(&self, req: EnableRequest, facts: &EnvFacts, catalog: &Catalog) -> Result<EnablePlan, CapabilityError>;
    fn status(&self, capability: Option<&str>, state: &InstalledState, catalog: &Catalog) -> Result<StatusReport, CapabilityError>;
}
```

`InstallSource::Auto` 的默认策略是优先使用预编译产物。只有开发场景或用户显式传入
`--from-source` 时，才进入 `Source`，并可复用 `build-all.sh` 或组件 Makefile。

`CapabilityView` JSON 形态：

```json
{
  "name": "agent-observability",
  "status": "available|degraded|unavailable|enabled",
  "priority": "P0",
  "reason": "CAP_BPF available",
  "advice": null,
  "components": ["agentsight"],
  "features": [
    {"name": "token_counting", "status": "available"},
    {"name": "ebpf_tracing", "status": "available"}
  ]
}
```

### 5.4 Plan 和 execution service

所有安装、卸载、更新都先转成 plan。

```rust
pub struct EnablePlan {
    pub operation_id: String,
    pub dry_run: bool,
    pub capabilities: Vec<CapabilityPlan>,
    pub steps: Vec<PlanStep>,
    pub warnings: Vec<PlanWarning>,
}

pub struct PlanStep {
    pub id: String,
    pub phase: PlanPhase,
    pub object: PlanObject,
    pub action: String,
    pub requires_privilege: bool,
    pub command_preview: Option<String>,
}

pub enum PlanPhase {
    Probe,
    Resolve,
    Build,
    Install,
    Configure,
    Adapter,
    Service,
    Verify,
    State,
}

pub trait Executor {
    fn execute(&self, plan: EnablePlan, ctx: &CliContext) -> Result<ExecutionReport, ExecutionError>;
}
```

第一阶段 `execute` 可以只支持 dry-run。真实写系统之前，必须实现 transaction、rollback、audit log、state lock。

### 5.5 Adapter service

```rust
pub struct AdapterScanRequest {
    pub include_paths: bool,
}

pub struct AdapterInstallRequest {
    pub component: String,
    pub framework: String,
    pub source: InstallSource,
}

pub trait AdapterService {
    fn scan(&self, facts: &EnvFacts) -> Result<Vec<DetectedFramework>, AdapterError>;
    fn list(&self, catalog: &Catalog, state: &InstalledState) -> Result<Vec<AdapterView>, AdapterError>;
    fn plan_install(&self, req: AdapterInstallRequest, catalog: &Catalog, facts: &EnvFacts) -> Result<AdapterPlan, AdapterError>;
}
```

实现建议：

- `adapter scan` 走 `anolisa-env` 的 framework probe。
- `adapter install/remove/status` 走 `TargetProvider` + planner + transaction。
- 旧 `scripts/anolisa-adapter-runner` 只能作为兼容后端，不作为产品主路径，也不能把 shell 细节泄漏到 CLI 用户语义。

## 6. Capability Manifest v2

文件路径：

```text
src/anolisa/manifests/capabilities/<capability>.toml
```

必填字段：

```toml
manifest_version = 2
anolisa_min_version = "0.1.0"

[capability]
name = "agent-observability"
description = "Agent behavior tracing and token attribution"
priority = "P0"
stability = "experimental" # experimental | beta | stable

[implementation]
components = ["agentsight"]
features.agentsight = ["token_counting", "ebpf_tracing"]

[requires_env]
os = "linux"
arch = ["x86_64", "aarch64"]

[[degrades]]
when = "linux_capabilities lacks CAP_BPF"
status = "unavailable"
reason = "requires CAP_BPF for eBPF tracing"
advice = "Enable on host or run privileged container with CAP_BPF."
```

字段说明：

| 字段 | 必填 | 说明 |
|---|---|---|
| `manifest_version` | 是 | schema 版本，第一阶段统一使用 2 |
| `anolisa_min_version` | 是 | 最低 CLI 版本 |
| `capability.name` | 是 | 用户可见 capability 名称 |
| `capability.description` | 是 | 用户可见描述 |
| `capability.priority` | 是 | `P0/P1/P2`，用于 roadmap 和 list 排序 |
| `capability.stability` | 是 | 成熟度 |
| `implementation.components` | 是 | 后端 component 列表 |
| `implementation.features.<component>` | 否 | 默认启用或推荐的 feature |
| `requires_env` | 否 | capability 级环境要求 |
| `degrades` | 否 | 降级或不可用规则 |

osbase 聚合建议：

- `sandbox` 不应引用不存在的 `osbase-sandbox`，除非实现 virtual component。
- 推荐 v2 表达为 backend selector：

```toml
[implementation]
components = ["sandbox-kata", "sandbox-firecracker", "sandbox-landlock"]
select = "any"

[[backends]]
name = "kata"
component = "sandbox-kata"
requires_env = { kvm_available = "true" }

[[backends]]
name = "landlock"
component = "sandbox-landlock"
requires_env = { landlock_abi = ">=1" }
```

## 7. Component Manifest v2

文件路径：

```text
src/anolisa/manifests/runtime/<component>.toml
src/anolisa/manifests/osbase/<component>.toml
```

推荐模板：

```toml
manifest_version = 2
anolisa_min_version = "0.1.0"

[component]
name = "agentsight"
version = "0.5.0"
layer = "runtime"              # runtime | osbase
domain = "observability"       # observability | cost | state | security | tools | sandbox | kernel
description = "Agent observability"
stability = "experimental"

[source]
path = "src/agentsight"
upstream = "workspace"

[distribution]
default_channel = "stable"
allowed_channels = ["stable", "beta", "nightly"]
index_ref = "builtin"               # builtin | remote | local
source_fallback = false             # source build only when user passes --from-source
checksum = "sha256"
signature = "cosign"

[[distribution.selectors]]
install_mode = "system"
os = ["alinux", "anolis", "rhel", "centos"]
pkg_base = "rpm"
preferred_artifact_types = ["rpm", "tar.gz"]

[[distribution.selectors]]
install_mode = "system"
os = ["ubuntu", "debian"]
pkg_base = "deb"
preferred_artifact_types = ["deb", "tar.gz"]

[[distribution.selectors]]
install_mode = "user"
os = ["linux"]
preferred_artifact_types = ["tar.gz"]

[[distribution.selectors]]
install_mode = "user"
os = ["macos"]
preferred_artifact_types = ["tar.gz", "zip"]

[build]
system = "make"                # cargo | make | npm | static | legacy-script
targets = ["agentsight"]
profile = "release"
pre_build = []

[build.toolchain]
rust = ">=1.80.0"
clang = ">=14"
libbpf = ">=0.8"

[[build.outputs]]
name = "agentsight"
path = "target/release/agentsight"
kind = "binary"

[install]
modes = ["system"]
services = ["agentsight.service"]

[[install.files]]
source = "target/release/agentsight"
dest = "{bindir}/agentsight"
mode = "0755"

[[install.capabilities]]
path = "{bindir}/agentsight"
caps = ["cap_bpf", "cap_perfmon"]
optional = true

[environment]
requires_os = "linux"
requires_arch = ["x86_64", "aarch64"]
requires_kernel = ">=5.8"
incompatible_env = []

[environment.requires_env]
btf_available = "true"
linux_capabilities = ["CAP_BPF"]

[dependencies]
build = ["rust>=1.80", "clang>=14", "libbpf-dev"]
runtime = ["kernel-headers"]
components = []

[[features]]
name = "token_counting"
label = "LLM Token metering"
default = true

[[features]]
name = "server"
label = "HTTP observability dashboard"
default = true

[features.requires_env]
port_available = "9090"

[[features]]
name = "ebpf_tracing"
label = "eBPF-based execution tracing"
default = true

[features.requires_env]
linux_capabilities = ["CAP_BPF"]
btf_available = "true"

[[health_checks]]
name = "binary"
kind = "command"
command = "{bindir}/agentsight --help"

[[health_checks]]
name = "service"
kind = "systemd"
unit = "agentsight.service"
optional = true
```

字段说明：

| 字段 | 必填 | 说明 |
|---|---|---|
| `manifest_version` | 是 | schema 版本 |
| `anolisa_min_version` | 是 | 最低 CLI 版本 |
| `component.name` | 是 | component 唯一名 |
| `component.version` | 是 | 应与实际组件版本同步 |
| `component.layer` | 是 | `runtime/osbase` |
| `component.domain` | 是 | 用于 capability 分类 |
| `source.path` | 是 | 相对仓库根目录的源码路径 |
| `distribution` | 是 | 预编译产物选择策略、允许 channel、校验和签名策略；具体 URL/checksum/signature 在 DistributionIndex 中声明 |
| `distribution.selectors` | 是 | 按 install mode、OS、arch/libc/pkg_base 选择 artifact type 的策略 |
| `build.system` | 是 | 构建后端 |
| `build.targets` | 否 | 产物名 |
| `build.outputs` | 是 | 构建产物路径，安装阶段只消费 outputs |
| `install.modes` | 是 | `user/system` |
| `install.files` | 是 | 安装文件映射 |
| `install.capabilities` | 否 | Linux file capabilities，例如 AgentSight setcap |
| `environment` | 否 | 环境要求 |
| `dependencies` | 否 | 构建/运行/component 依赖 |
| `features` | 否 | feature 开关 |
| `adapters` | 否 | 外部 agent runtime 集成 |
| `health_checks` | 否 | status/doctor 使用 |

合法 placeholder：

| Placeholder | 含义 |
|---|---|
| `{bindir}` | binary 安装目录 |
| `{libexecdir}` | helper/libexec 目录 |
| `{datadir}` | shared data 目录 |
| `{sysconfdir}` | config 目录 |
| `{statedir}` | state 目录 |
| `{logdir}` | log 目录 |
| `{runtimedir}` | socket/pid runtime 目录 |
| `{cachedir}` | cache 目录 |
| `{component}` | 当前 component name |

禁止使用 `{share_dir}` 这种未定义 placeholder。

## 8. DistributionIndex v1

核心口径：

- component manifest 只声明“这个组件是什么、支持哪些安装形态、偏好哪些 artifact type”。
- DistributionIndex 声明“某个版本在某个 channel 上有哪些具体可下载产物”。
- GitHub Release 上挂 RPM 是合理的 artifact backend，但不是唯一标准。
- `anolisa` 只依赖 DistributionIndex 契约，不把 GitHub、yum repo、OSS/CDN、内网制品库硬编码进命令逻辑。

推荐路径：

```text
src/anolisa/manifests/distribution/index.toml         # built-in fallback
https://anolisa.example.com/index/stable/index.toml   # public remote index
https://<private-mirror>/anolisa/index/stable.toml    # enterprise/private index
```

推荐 schema：

```toml
schema_version = 1
channel = "stable"
generated_at = "2026-06-01T10:00:00Z"
expires_at = "2026-07-01T10:00:00Z"
publisher = "anolisa"
signature = "cosign"

[[components]]
name = "agentsight"
version = "0.5.0"
manifest_digest = "sha256:..."

[[components.artifacts]]
artifact_id = "agentsight-0.5.0-alinux4-x86_64-rpm"
type = "rpm"                         # rpm | deb | tar.gz | zip | npm | oci | source
backend = "github-release"           # github-release | yum-repo | apt-repo | aliyun-oss | internal-registry | local-file
url = "https://github.com/casparant/anolisa/releases/download/agentsight-v0.5.0/agentsight-0.5.0.alinux4.x86_64.rpm"
os = "alinux"
os_version = ">=4"
arch = "x86_64"
libc = "glibc"
pkg_base = "rpm"
install_modes = ["system"]
sha256 = "..."
signature_url = "https://github.com/casparant/anolisa/releases/download/agentsight-v0.5.0/agentsight-0.5.0.alinux4.x86_64.rpm.sig"
size = 12345678

[components.artifacts.dependencies]
rpm = ["kernel-headers"]

[[components.artifacts]]
artifact_id = "agentsight-0.5.0-linux-x86_64-tar"
type = "tar.gz"
backend = "aliyun-oss"
url = "https://anolisa.oss-cn-hangzhou.aliyuncs.com/agentsight/0.5.0/agentsight-0.5.0-linux-x86_64.tar.gz"
os = "linux"
arch = "x86_64"
libc = "glibc"
install_modes = ["user", "system"]
sha256 = "..."
signature_url = "https://anolisa.oss-cn-hangzhou.aliyuncs.com/agentsight/0.5.0/agentsight-0.5.0-linux-x86_64.tar.gz.sig"
size = 9876543
```

字段口径：

| 字段 | 必填 | 说明 |
|---|---|---|
| `schema_version` | 是 | DistributionIndex schema 版本 |
| `channel` | 是 | `stable/beta/nightly/dev` |
| `generated_at` | 是 | 生成时间，用于排查版本漂移 |
| `expires_at` | 否 | 可选过期时间，企业内网 index 可以不设置 |
| `publisher` | 是 | 发布主体 |
| `signature` | 是 | index 签名方式 |
| `components.name/version` | 是 | 对应 component manifest |
| `components.manifest_digest` | 是 | component manifest digest，防止 manifest 与 artifact 不一致 |
| `artifacts.type` | 是 | 产物类型，不等于下载来源 |
| `artifacts.backend` | 是 | 下载/安装 backend，例如 GitHub Release、yum repo、OSS、internal registry |
| `artifacts.url` | 是 | 具体 artifact URL 或 repo locator |
| `artifacts.os/arch` | 是 | resolver 选择依据 |
| `artifacts.libc/pkg_base` | 条件必填 | Linux artifact 通常需要 `libc`；系统包 artifact 需要 `pkg_base` |
| `artifacts.install_modes` | 是 | `user/system` |
| `artifacts.sha256` | 是 | artifact 校验 |
| `artifacts.signature_url` | 是 | artifact 签名 |
| `artifacts.dependencies` | 否 | backend-specific 依赖，例如 rpm/deb/npm 包依赖 |

resolver 规则：

1. 读取 component manifest 的 `distribution.selectors`。
2. 加载 signed DistributionIndex，来源可以是 built-in、remote、private mirror 或 local override。
3. 按 component、version、channel、install mode、OS、arch、libc、pkg_base 过滤 artifact。
4. 按 selector 中的 `preferred_artifact_types` 排序。
5. 下载并校验 index signature、artifact sha256、artifact signature。
6. 生成 install plan，写入 state、central log、backup/rollback boundary。
7. 没有匹配 artifact 时返回 `ARTIFACT_NOT_FOUND` 和可执行建议；只有用户显式传 `--from-source` 才进入 build backend。

GitHub Release RPM 的使用边界：

| 场景 | 结论 |
|---|---|
| 开源 MVP / 早期试用 | 合理，可以作为 `type=rpm, backend=github-release` |
| Alinux/Anolis/RHEL system-mode | 合理，但需要 checksum/signature 和依赖声明 |
| 批量企业更新 | 建议切到 yum repo、OSS/CDN 或内网制品库，由同一个 DistributionIndex 指向 |
| rootless/container/user-mode | 不应依赖 RPM，优先 tar.gz/zip/OCI artifact |
| macOS | 不应依赖 RPM，优先 tar.gz/zip，后续可补 homebrew |

## 9. Adapter Manifest 字段

component manifest 内置 adapter 声明：

```toml
[[adapters]]
framework = "openclaw"
kind = "third-party"           # first-party | third-party | protocol
plugin_id = "tokenless-openclaw"
source = "adapters/tokenless/openclaw"
dest = "{datadir}/adapters/{component}/openclaw/"

[adapters.detect]
binary = "openclaw"
paths = ["~/.openclaw"]

[adapters.actions]
detect = "openclaw/scripts/detect.sh"
install = "openclaw/scripts/install.sh"
uninstall = "openclaw/scripts/uninstall.sh"
status = "openclaw/scripts/detect.sh"

[adapters.env]
OPENCLAW_HOME = "{framework_home}"
ANOLISA_COMPONENT = "{component}"
ANOLISA_INSTALL_MODE = "{install_mode}"
```

规则：

- `first-party` adapter 默认随 capability 安装。
- `third-party` adapter 只有 `--with-adapter` 或 `adapter install` 才安装。
- `protocol` adapter 按标准协议路径发布，例如 MCP server config。
- detect hint 只是 manifest 注解，权威 framework 探测规则在 `anolisa-env`.
- adapter action 第一阶段可以调用现有 shell 脚本；后续再逐步收敛到 Rust executor。

## 10. anolisa-cli 如何使用这些字段

### 9.1 `anolisa env`

调用链：

```text
CliContext -> EnvService.detect -> EnvFacts -> formatter
```

使用字段：

- 不读 component manifest。
- 只输出当前机器事实。
- `--json` 输出完整 EnvFacts。

### 9.2 `anolisa list`

调用链：

```text
CatalogService.load_builtin
EnvService.detect
CapabilityService.list
formatter
```

使用字段：

- capability manifest 的 `implementation/requires_env/degrades/priority`.
- component manifest 的 `environment/features/adapters`.
- 输出 capability 维度的状态，不要求用户理解 component。

### 9.3 `anolisa enable <capability>`

调用链：

```text
Catalog + EnvFacts
-> CapabilityService.plan_enable
-> dependency planner
-> build/install/adapter/service/verify plan
-> dry-run formatter or Executor
```

使用字段：

- capability manifest 决定 component 和默认 feature。
- component manifest 决定构建、安装、依赖、服务、健康检查。
- adapter manifest 决定 external framework 集成。
- environment 字段决定 available/degraded/unavailable。

AgentSight P0 的要求：

- `anolisa enable agent-observability --dry-run` 必须明确告诉用户是否满足 BTF、CAP_BPF、CAP_PERFMON、kernel、arch。
- 在 container/rootless 场景中，如果不能启用 eBPF tracing，要返回降级或不可用原因。
- 即使不可用，也要能输出“建议在宿主机安装”这类修复建议。

### 9.4 `anolisa runtime *`

Runtime surface 直接操作 component。

```text
runtime list     -> component catalog
runtime build    -> build backend
runtime install  -> install plan/executor
runtime status   -> installed state + health checks
```

这里可以暴露 `agentsight/tokenless/ws-ckpt/agent-sec-core` 等 component 名称。

### 9.5 `anolisa adapter *`

Adapter surface 直接操作 component + framework。

```text
adapter scan                 -> EnvService framework probe
adapter list                 -> catalog adapters + installed state
adapter install COMP FW      -> AdapterService.plan_install -> TargetProvider planner
adapter remove COMP FW       -> uninstall action
```

短期优先支持：

- `tokenless -> openclaw/hermes`
- `agent-sec-core -> openclaw/hermes`
- `ws-ckpt -> openclaw/hermes`
- `os-skills -> openclaw/hermes`

AgentSight 当前没有 OpenClaw/Hermes adapter，不应强行声明 adapter；它是观测入口，先通过 host/guest/container 观测策略接入。

## 11. JSON 输出契约

所有 `--json` 顶层形态统一：

```json
{
  "ok": true,
  "schema_version": 1,
  "command": "list",
  "data": {},
  "warnings": []
}
```

dry-run plan 形态：

```json
{
  "ok": true,
  "schema_version": 1,
  "command": "enable",
  "data": {
    "dry_run": true,
    "capabilities": ["agent-observability"],
    "steps": [
      {
        "id": "install-agentsight",
        "phase": "install",
        "object": {"kind": "component", "name": "agentsight"},
        "action": "install binary",
        "requires_privilege": true,
        "command_preview": "install target/release/agentsight {bindir}/agentsight"
      }
    ]
  },
  "warnings": []
}
```

## 12. 分发与编译左移

已确认的产品化方向：

- 官网安装脚本只负责安装 `anolisa` CLI 本体。
- `anolisa` CLI 本体以预编译二进制分发，安装脚本按 OS/arch/libc 下载对应 release asset。
- 后续组件默认也直接使用预编译产物，例如 release asset、RPM、DEB、tarball、OCI artifact 或内部制品仓库。
- 组件 manifest 不绑定具体下载源；具体 artifact URL、checksum、signature、backend 统一由 DistributionIndex 声明。
- GitHub Release RPM 是合理的早期 backend，但正式契约必须允许 yum repo、apt repo、OSS/CDN、internal registry 和 local file。
- 编译左移到 CI/release 阶段完成，客户机器默认不做源码编译。
- `build-all.sh` 只保留为 develop 场景、CI 兼容入口和 `--from-source` 后端，不再作为客户安装入口。

官网安装脚本契约：

```bash
curl -fsSL https://anolisa.example.com/install.sh | sh
```

安装脚本只做这些事：

1. 探测 `os/arch/libc`.
2. 下载匹配的 `anolisa` 二进制。
3. 校验 checksum/signature。
4. 安装到 `~/.local/bin/anolisa` 或 `/usr/local/bin/anolisa`。
5. 输出下一步命令，例如 `anolisa env`、`anolisa list`。

安装脚本不做这些事：

- 不自动安装 AgentSight、Tokenless、Sec-Core、ws-ckpt。
- 不运行 `build-all.sh`。
- 不修改 OpenClaw/Hermes/Codex/Claude Code 配置。
- 不写入组件 installed state。

组件分发策略：

| 场景 | 默认来源 | 命令 |
|---|---|---|
| 生产/客户环境 | 预编译产物或系统包 | `anolisa enable <capability>` |
| 开发调试 | 源码构建 | `anolisa runtime build <component>` |
| 强制源码安装 | source build backend，必要时兼容 build-all | `anolisa enable <capability> --from-source` |
| adapter 快速适配 | 已安装组件产物 + adapter action | `anolisa adapter install <component> <target>` |

`anolisa enable` 的默认解析顺序：

```text
1. 读取 component manifest 的 `distribution.selectors`
2. 加载并校验 DistributionIndex
3. 按 install mode、os、arch、libc、pkg_base、channel 选择预编译产物
4. 校验 artifact checksum/signature
5. 安装文件、服务、adapter、state
6. 只有 --from-source 时进入 build backend
```

这意味着 build 字段仍然重要，但它服务于开发、CI、应急源码构建和产物生产，不是客户首选路径。

## 13. 开发分工建议

| 方向 | 负责人类型 | 输入 | 输出 |
|---|---|---|---|
| CLI context/formatter | CLI 同学 | clap command tree | `CliContext`、human/json formatter、错误输出 |
| Manifest schema/lint | core 同学 | v2 字段定义 | catalog loader、schema tests、lint |
| Env probe/gate | env 同学 | EnvFacts 字段 | real probe runner、gate evaluator |
| Capability planner | core 同学 | catalog + facts | list/status/enable dry-run plan |
| AgentSight P0 | runtime 同学 | agentsight manifest + install needs | `agent-observability` plan/status/doctor |
| TargetProvider bridge | adapter 同学 | target provider spec | `adapter scan/list/install --dry-run` |
| Build/install backend | platform/build 同学 | DistributionIndex、distribution/build/install fields | prebuilt installer、build-all develop backend、cargo/make/npm/static backend |
| Tests | 各模块 owner | 接口契约 | schema、CLI JSON、dry-run snapshot tests |

建议并行顺序：

1. 先合入 `CliContext` 和统一 JSON envelope。
2. 同时推进 Manifest v2 schema 和 EnvFacts real probe。
3. 然后接 `list` 与 `enable --dry-run`。
4. 最后把 AgentSight P0 和 TargetProvider adapter 机制接成两条可演示链路。

## 14. 待决策问题

| 问题 | 建议 | 状态 |
|---|---|---|
| Manifest v2 是否现在引入 | 现在引入；manifest 描述组件能力、安装选择策略和 build/install 契约，不承载具体下载 URL | 已确认 |
| DistributionIndex 是否现在引入 | 现在引入；具体 artifact URL/checksum/signature/backend 进入 DistributionIndex | 已确认 |
| GitHub Release RPM 是否合理 | 合理，但只是 `type=rpm, backend=github-release` 的一种 backend，不作为唯一标准 | 已确认 |
| `sandbox/os-security` 是否用 virtual component | 建议不用虚拟 component，改用 backend selector | 待定 |
| AgentSight 是否 system-only | P0 可先 system-only，但 manifest 需要声明 rootless/container 降级策略 | 待定 |
| `build-all.sh` 接入方式 | 只作为 develop/source-build backend，不作为客户安装入口 | 已确认 |
| 编译左移和预编译分发 | 官网安装脚本下载 `anolisa` 二进制；组件默认用预编译产物 | 已确认 |
| adapter action 是否继续 shell | 第一阶段继续 shell，Rust 只负责编排和状态 | 待定 |
| 第三方 manifest 是否支持 | 第一阶段不支持，只支持内置签名 manifest | 已建议 |

## 15. 本周最小交付

本周不要追求真实安装全量组件，先把框架跑通：

```text
Day 1:
  - CliContext
  - JSON envelope
  - Catalog loader split capability/component
  - Manifest v2 lint skeleton
  - distribution 字段 schema 和默认 prebuilt source 选择

Day 2:
  - EnvService real probes
  - anolisa env/list --json
  - Capability available/degraded/unavailable

Day 3:
  - agent-observability dry-run plan
  - AgentSight manifest version/path/arch 修正
  - CAP_BPF/BTF/rootless/container 降级输出

Day 4:
  - adapter scan
  - adapter install --dry-run 接 TargetProvider planner
  - runtime list/status catalog 化
  - install.sh 下载 anolisa 二进制的接口草案

Day 5:
  - schema tests
  - CLI JSON snapshot tests
  - 给老板演示 env/list/enable agent-observability --dry-run
```

最终演示命令：

```bash
cd src/anolisa
cargo test --workspace
cargo run -- --json env
cargo run -- --json list
cargo run -- --json enable agent-observability --dry-run
cargo run -- --json adapter scan
cargo run -- --json adapter install tokenless openclaw --dry-run
```
