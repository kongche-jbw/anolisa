#!/usr/bin/env bash
# ============================================================
# agenticos2openclaw.sh — AgenticOS for OpenClaw 一键集成脚本
# ============================================================
set -euo pipefail

if [[ "${BASH_VERSINFO[0]:-0}" -lt 4 ]]; then
  echo "Error: bash 4+ required (current: $BASH_VERSION)" >&2
  echo "Tip: on CentOS/RHEL: yum install -y bash || dnf install -y bash" >&2
  exit 1
fi

readonly SCRIPT_VERSION="0.2.0"
readonly SCRIPT_NAME="$(basename "$0")"

# ============================================================
# [Section 1] 全局常量与路径变量（均可通过环境变量覆盖）
# ============================================================

# AgenticOS 组件根路径（anolisa 是 AgenticOS 的产品名）
AGENTICOS_BASE="${AGENTICOS_BASE:-/usr/share/anolisa}"
# Runtime 组件的 Skill 输出目录
RUNTIME_SKILLS_DIR_USER_SET="${RUNTIME_SKILLS_DIR+x}"
RUNTIME_SKILLS_DIR="${RUNTIME_SKILLS_DIR:-$AGENTICOS_BASE/runtime/skills}"
# sec-core OpenClaw 集成一键部署脚本
SEC_CORE_DEPLOY_SH="${SEC_CORE_DEPLOY_SH:-/opt/agent-sec/openclaw-plugin/scripts/deploy.sh}"
# OpenClaw 配置
OPENCLAW_HOME="${OPENCLAW_HOME:-$HOME/.openclaw}"
OPENCLAW_CONFIG="${OPENCLAW_CONFIG:-$OPENCLAW_HOME/openclaw.json}"
OPENCLAW_SKILLS_DIR="${OPENCLAW_SKILLS_DIR:-$OPENCLAW_HOME/skills}"
STATE_FILE="${STATE_FILE:-$OPENCLAW_HOME/agenticos-state.json}"
BACKUP_DIR="${BACKUP_DIR:-$OPENCLAW_HOME/backups}"

# ============================================================
# [Section 2] 工具函数
# ============================================================

# --- 颜色 ---
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[0;33m'
BLUE='\033[0;34m'; CYAN='\033[0;36m'; BOLD='\033[1m'; NC='\033[0m'

# --- 日志 ---
log_info()    { echo -e "${GREEN}[✓]${NC} $*"; }
log_warn()    { echo -e "${YELLOW}[!]${NC} $*"; }
log_error()   { echo -e "${RED}[✗]${NC} $*" >&2; }
log_step()    { echo -e "\n${BOLD}${BLUE}▸ $*${NC}"; }
log_skip()    { echo -e "${CYAN}[−]${NC} $*"; }
log_verbose() { [[ "${VERBOSE:-0}" == "1" ]] && echo -e "    $*" || true; }

print_divider() { echo "    ----------------------------------------"; }

die() { log_error "$*"; exit 1; }

CURRENT_MODULE=""
declare -A MODULE_EXEC_CMDS

append_module_exec_cmd() {
  local module="$1" cmd="$2"
  [[ -z "$module" || -z "$cmd" ]] && return 0

  if [[ -z "${MODULE_EXEC_CMDS[$module]:-}" ]]; then
    MODULE_EXEC_CMDS["$module"]="$cmd"
  else
    MODULE_EXEC_CMDS["$module"]+=$'\n'"$cmd"
  fi
}

run_cmd() {
  local -a cmd=("$@")
  local rendered=""
  local part

  for part in "${cmd[@]}"; do
    rendered+="${rendered:+ }$(printf '%q' "$part")"
  done

  if [[ -n "${CURRENT_MODULE:-}" ]]; then
    append_module_exec_cmd "$CURRENT_MODULE" "$rendered"
  fi

  "${cmd[@]}"
}

# --- JSON 操作（依赖 jq） ---
json_read() {
  local file="$1" key="$2"
  [[ -f "$file" ]] && jq -r "$key // empty" "$file" 2>/dev/null || echo ""
}

json_write() {
  local file="$1" jq_expr="$2"
  local tmp
  tmp="$(mktemp)"
  if jq "$jq_expr" "$file" > "$tmp" 2>/dev/null && jq empty "$tmp" 2>/dev/null; then
    mv "$tmp" "$file"
  else
    rm -f "$tmp"
    log_error "JSON write failed: $jq_expr"
    return 1
  fi
}

json_write_with_args() {
  local file="$1"
  shift
  local tmp
  tmp="$(mktemp)"
  if jq "$@" "$file" > "$tmp" 2>/dev/null && jq empty "$tmp" 2>/dev/null; then
    mv "$tmp" "$file"
  else
    rm -f "$tmp"
    log_error "JSON write failed"
    return 1
  fi
}

json_init() {
  local file="$1"
  mkdir -p "$(dirname "$file")"
  [[ -f "$file" ]] || echo '{}' > "$file"
}

normalize_openclaw_plugin_config() {
  [[ -f "$OPENCLAW_CONFIG" ]] || return 0

  local tokenless_plugin_dir="${MODULE_LOCAL_PATH[token-less]:-}"
  local jq_expr='.plugins.enabled = true | .plugins.allow = (.plugins.allow // [] | map(select(. != "plugin")) | unique)'

  if [[ -n "$tokenless_plugin_dir" ]]; then
    json_write_with_args "$OPENCLAW_CONFIG" --arg plugin_dir "$tokenless_plugin_dir" \
      "$jq_expr | if .plugins.entries[\"tokenless-openclaw\"]? != null then .plugins.load.paths = ([.plugins.load.paths // [] | .[] | select(. != \$plugin_dir)]) else . end"
  else
    json_write_with_args "$OPENCLAW_CONFIG" "$jq_expr"
  fi

  log_info "已归一化 OpenClaw plugin 配置（清理 plugin 残留项与 tokenless 重复加载路径）"
}

refresh_openclaw_plugin_registry() {
  if ! command -v openclaw &>/dev/null; then
    return 0
  fi

  if run_cmd openclaw plugins registry --refresh &>/dev/null; then
    log_info "OpenClaw plugin registry 已刷新"
  else
    log_warn "OpenClaw plugin registry 刷新失败，请稍后手动执行: openclaw plugins registry --refresh"
    return 1
  fi
}

remove_plugin_config_entries() {
  local plugin_id="$1"
  local plugin_dir="${2:-}"

  [[ -f "$OPENCLAW_CONFIG" ]] || return 0

  if [[ -n "$plugin_dir" ]]; then
    json_write_with_args "$OPENCLAW_CONFIG" --arg plugin_id "$plugin_id" --arg plugin_dir "$plugin_dir" \
      'del(.plugins.entries[$plugin_id]) |
       .plugins.allow = (.plugins.allow // [] | map(select(. != $plugin_id))) |
       .plugins.load.paths = ([.plugins.load.paths // [] | .[] | select(. != $plugin_dir)])'
  else
    json_write_with_args "$OPENCLAW_CONFIG" --arg plugin_id "$plugin_id" \
      'del(.plugins.entries[$plugin_id]) |
       .plugins.allow = (.plugins.allow // [] | map(select(. != $plugin_id)))'
  fi
}

cleanup_skvm_skill_links() {
  [[ -d "$OPENCLAW_SKILLS_DIR" ]] || return 0

  local skvm_originals_dir="${MODULE_LOCAL_PATH[skvm]}/originals"
  local removed=0

  while IFS= read -r -d '' link_path; do
    local target_path
    target_path="$(readlink "$link_path" 2>/dev/null || true)"

    if [[ "$link_path" == *.skvm-tmp.* ]] || [[ -n "$target_path" && "$target_path" == "$skvm_originals_dir"/* ]]; then
      rm -f "$link_path"
      removed=$((removed + 1))
    fi
  done < <(find "$OPENCLAW_SKILLS_DIR" -maxdepth 1 -type l -print0 2>/dev/null)

  if [[ "$removed" -gt 0 ]]; then
    log_info "已清理 $removed 个 skvm 残留链接"
  fi
}

finalize_full_rollback() {
  json_init "$OPENCLAW_CONFIG"

  remove_plugin_config_entries "agent-sec"
  remove_plugin_config_entries "tokenless-openclaw" "${MODULE_LOCAL_PATH[token-less]}"
  cleanup_skvm_skill_links
  normalize_openclaw_plugin_config
  refresh_openclaw_plugin_registry || true
}

# --- 备份/恢复 ---
backup_file() {
  local file="$1"
  [[ ! -f "$file" ]] && return 0
  mkdir -p "$BACKUP_DIR"
  local bak="$BACKUP_DIR/$(basename "$file").bak.$(date +%Y%m%d%H%M%S)"
  cp "$file" "$bak"
  log_verbose "Backed up: $file -> $bak"
  echo "$bak"
}

restore_backup() {
  local latest
  latest="$(ls -t "${BACKUP_DIR}/openclaw.json.bak."* 2>/dev/null | head -1)"
  if [[ -n "$latest" ]]; then
    run_cmd cp "$latest" "$OPENCLAW_CONFIG"
    log_info "Restored config from: $latest"
  else
    log_warn "No backup found to restore"
  fi
}

# --- 状态管理 ---
state_init() {
  json_init "$STATE_FILE"
  local ver
  ver="$(json_read "$STATE_FILE" '.version')"
  if [[ "$ver" == "" ]]; then
    echo '{"version":1,"modules":{}}' > "$STATE_FILE"
  fi
}

state_set_module() {
  local name="$1" status="$2" error="${3:-}"
  local ts
  ts="$(date -Iseconds)"
  if [[ -n "$error" ]]; then
    json_write_with_args "$STATE_FILE" \
      --arg name "$name" --arg status "$status" --arg ts "$ts" --arg error "$error" \
      '.modules[$name] = {status:$status,ts:$ts,error:$error}'
  else
    json_write_with_args "$STATE_FILE" \
      --arg name "$name" --arg status "$status" --arg ts "$ts" \
      '.modules[$name] = {status:$status,ts:$ts}'
  fi
}

state_get_module() {
  local name="$1" field="${2:-status}"
  json_read "$STATE_FILE" ".modules[\"$name\"].$field"
}

state_list_modules() {
  json_read "$STATE_FILE" '.modules | keys[]'
}

# --- RPM 依赖保障 ---
ensure_rpm() {
  local pkg="$1"
  [[ -z "$pkg" ]] && return 0
  if run_cmd rpm -q "$pkg" &>/dev/null; then
    return 0
  fi
  if [[ "${AUTO_INSTALL_RPM:-0}" -eq 1 ]]; then
    log_info "正在安装 RPM 依赖: $pkg ..."
    if run_cmd dnf install -y "$pkg" &>/dev/null; then
      log_info "$pkg 安装成功"
      return 0
    else
      log_error "$pkg 安装失败"
      return 1
    fi
  else
    log_error "RPM 未安装: $pkg"
    log_error "请先安装: dnf install -y $pkg （或使用 --install-deps 自动安装）"
    return 1
  fi
}

# --- 文件下载 ---
download_file() {
  local url="$1" dest="$2"
  if command -v curl &>/dev/null; then
    curl -fsSL "$url" -o "$dest"
  elif command -v wget &>/dev/null; then
    wget -q "$url" -O "$dest"
  else
    die "Neither curl nor wget found"
  fi
}

# --- Skill 元数据解析 ---
parse_skill_metadata() {
  local skill_dir="$1"
  local skill_md="$skill_dir/SKILL.md"
  local _name="" _desc=""

  if [[ -f "$skill_md" ]]; then
    local in_front=0
    local in_multiline_desc=0
    while IFS= read -r line; do
      if [[ "$line" == "---" ]]; then
        if [[ "$in_front" -eq 0 ]]; then
          in_front=1
          continue
        else
          break
        fi
      fi
      if [[ "$in_front" -eq 1 ]]; then
        # 处理多行描述签缩行（description: | 模式）
        if [[ "$in_multiline_desc" -eq 1 ]]; then
          # 如果行以空格/tab 开头，属于多行内容
          if [[ "$line" =~ ^[[:space:]]+(.*) ]]; then
            local content="${BASH_REMATCH[1]}"
            # 第一行直接赋值，后续行用空格拼接
            if [[ -z "$_desc" ]]; then
              _desc="$content"
            else
              _desc="$_desc $content"
            fi
            continue
          else
            # 缩进结束，退出多行模式
            in_multiline_desc=0
          fi
        fi
        if [[ "$line" =~ ^name:[[:space:]]*(.*)$ ]]; then
          _name="${BASH_REMATCH[1]}"
          _name="${_name#\"}"; _name="${_name%\"}"
          _name="${_name#\'}"; _name="${_name%\'}"
        elif [[ "$line" =~ ^description:[[:space:]]*\|[[:space:]]*$ ]]; then
          # 多行块标记（description: |），开始收集后续行
          _desc=""
          in_multiline_desc=1
        elif [[ "$line" =~ ^description:[[:space:]]*[\"\'\`](.*)[\"\'\`]$ ]]; then
          _desc="${BASH_REMATCH[1]}"
        elif [[ "$line" =~ ^description:[[:space:]]*(.*)$ ]]; then
          _desc="${BASH_REMATCH[1]}"
          _desc="${_desc#\"}"; _desc="${_desc%\"}"
          _desc="${_desc#\'}"; _desc="${_desc%\'}"
        fi
      fi
    done < "$skill_md"
  fi

  [[ -z "$_name" ]] && _name="$(basename "$skill_dir")"
  [[ -z "$_desc" ]] && _desc="无描述"

  SKILL_META_NAME="$_name"
  SKILL_META_DESC="$_desc"
}

# --- Skill 交互式选择（含范围选择支持，可在模块确认阶段预调用）---
# 输出: SELECTED_SKILL_PATHS 全局数组
select_skills_interactive() {
  local -a skill_paths=("$@")

  if [[ ${#skill_paths[@]} -eq 0 ]]; then
    SELECTED_SKILL_PATHS=()
    return 1
  fi

  if [[ ${#skill_paths[@]} -eq 1 ]]; then
    SELECTED_SKILL_PATHS=("${skill_paths[@]}")
    return 0
  fi

  # 非 tty 环境（如管道/自动化）默认全选
  if [[ ! -t 0 ]]; then
    SELECTED_SKILL_PATHS=("${skill_paths[@]}")
    return 0
  fi

  local total=${#skill_paths[@]}
  echo ""
  echo -e "  ${BOLD}选择要安装的 Skills（共 ${total} 个）:${NC}"
  echo -e "  ${CYAN}支持: 单选 \"1 3\", 范围 \"1-5\", 全选 \"a\", 取消 \"0\", 混合 \"1-3 6 8\"${NC}"
  echo ""

  local i=1
  for skill_path in "${skill_paths[@]}"; do
    parse_skill_metadata "$skill_path"
    local short_desc="$SKILL_META_DESC"
    # 截断过长描述（超过 60 字符）
    if [[ ${#short_desc} -gt 60 ]]; then
      short_desc="${short_desc:0:57}..."
    fi
    printf "    %3d) %-22s %s\n" "$i" "[$SKILL_META_NAME]" "$short_desc"
    i=$((i + 1))
  done

  echo ""
  local choices
  read -rp "  请选择 [编号/范围/a/0]: " choices

  SELECTED_SKILL_PATHS=()
  if [[ "$choices" == "a" || "$choices" == "A" ]]; then
    SELECTED_SKILL_PATHS=("${skill_paths[@]}")
  elif [[ "$choices" == "0" ]]; then
    SELECTED_SKILL_PATHS=()
  else
    # 解析混合输入：支持 "1-5 7 9" 格式
    local -A _seen=()
    for token in $choices; do
      if [[ "$token" =~ ^([0-9]+)-([0-9]+)$ ]]; then
        local from=${BASH_REMATCH[1]} to=${BASH_REMATCH[2]}
        for ((n=from; n<=to; n++)); do
          local idx=$((n - 1))
          if [[ "$idx" -ge 0 && "$idx" -lt $total && -z "${_seen[$idx]:-}" ]]; then
            SELECTED_SKILL_PATHS+=("${skill_paths[$idx]}")
            _seen[$idx]=1
          fi
        done
      elif [[ "$token" =~ ^[0-9]+$ ]]; then
        local idx=$((token - 1))
        if [[ "$idx" -ge 0 && "$idx" -lt $total && -z "${_seen[$idx]:-}" ]]; then
          SELECTED_SKILL_PATHS+=("${skill_paths[$idx]}")
          _seen[$idx]=1
        fi
      fi
    done
  fi
}

# --- Skill 迁移：从源目录复制到 OpenClaw skills 目录 ---
migrate_skills() {
  local src_dir="$1"
  local dest_dir="$2"
  local select_mode="${3:-0}"
  local count=0

  if [[ ! -d "$src_dir" ]]; then
    log_error "Skill 源目录不存在: $src_dir"
    return 1
  fi

  mkdir -p "$dest_dir"

  # 收集所有可用 skills
  local -a all_skills=()
  if [[ -f "$src_dir/SKILL.md" ]]; then
    all_skills+=("$src_dir")
  else
    for skill_path in "$src_dir"/*/; do
      [[ -d "$skill_path" ]] || continue
      if [[ -f "$skill_path/SKILL.md" ]]; then
        all_skills+=("$skill_path")
      fi
    done
  fi

  if [[ ${#all_skills[@]} -eq 0 ]]; then
    log_warn "No skills found in $src_dir"
    return 1
  fi

  # 如果调用方已预选（PRESELECTED_SKILL_PATHS 有内容），直接使用，跳过交互
  if [[ ${#PRESELECTED_SKILL_PATHS[@]} -gt 0 ]]; then
    SELECTED_SKILL_PATHS=("${PRESELECTED_SKILL_PATHS[@]}")
  elif [[ "$select_mode" == "1" && ${#all_skills[@]} -gt 1 ]]; then
    # 交互式选择（仅当 select_mode=1 且存在多个 skill 且处于 tty 时）
    select_skills_interactive "${all_skills[@]}"
  else
    SELECTED_SKILL_PATHS=("${all_skills[@]}")
  fi

  if [[ ${#SELECTED_SKILL_PATHS[@]} -eq 0 ]]; then
    log_warn "用户未选择任何 skill"
    return 1
  fi

  for skill_path in "${SELECTED_SKILL_PATHS[@]}"; do
    local skill_name
    skill_name="$(basename "$skill_path")"
    run_cmd cp -r "$skill_path" "$dest_dir/$skill_name"
    log_verbose "Migrated skill: $skill_name"
    count=$((count + 1))
  done

  log_info "Migrated $count skills: $src_dir -> $dest_dir"
  return 0
}

# 清理已迁移的 skills
unmigrate_skills() {
  local src_dir="$1"
  local dest_dir="$2"

  if [[ ! -d "$src_dir" ]] || [[ ! -d "$dest_dir" ]]; then
    return 0
  fi

  # 单目录模式：src_dir 根目录下直接有 SKILL.md
  if [[ -f "$src_dir/SKILL.md" ]]; then
    local skill_name
    skill_name="$(basename "$src_dir")"
    if [[ -d "$dest_dir/$skill_name" ]]; then
      rm -rf "$dest_dir/$skill_name"
      log_verbose "Removed skill: $skill_name"
    fi
  else
    # 多子目录模式：遍历 src_dir 下的子目录
    for skill_path in "$src_dir"/*/; do
      [[ -d "$skill_path" ]] || continue
      local skill_name
      skill_name="$(basename "$skill_path")"
      if [[ -d "$dest_dir/$skill_name" ]]; then
        rm -rf "$dest_dir/$skill_name"
        log_verbose "Removed skill: $skill_name"
      fi
    done
  fi
}

# --- 交互 UI ---
prompt_choice() {
  local prompt="$1"; shift
  local default="${1:-}"
  local answer
  read -rp "$prompt " answer
  answer="${answer:-$default}"
  echo "$answer"
}

prompt_yesno() {
  local prompt="$1" default="${2:-Y}"
  local answer
  read -rp "$prompt [$default] " answer
  answer="${answer:-$default}"
  [[ "$answer" =~ ^[Yy] ]]
}

# ============================================================
# [Section 3] 前置检测
# ============================================================

prerequisites_check() {
  local failed=0

  log_step "前置检测"

  # OS
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  if [[ "$os" != "Linux" ]]; then
    log_warn "Unsupported OS: $os (Linux required)"
    failed=$((failed + 1))
  fi
  log_info "系统: $os ($arch)"

  # Root
  if [[ "$(id -u)" -ne 0 ]]; then
    log_warn "非 root 用户，部分模块（RPM 安装）可能需要 sudo"
  fi

  # jq
  if ! command -v jq &>/dev/null; then
    log_error "jq 未安装（必需）"
    failed=$((failed + 1))
  else
    log_info "jq: $(jq --version)"
  fi

  # npm
  if ! command -v npm &>/dev/null; then
    log_warn "npm 未安装，OpenClaw 安装可能失败"
  else
    log_info "npm: $(npm --version)"
  fi

  # Anolisa 基础目录
  if [[ -d "$AGENTICOS_BASE" ]]; then
    log_info "Anolisa 目录: $AGENTICOS_BASE"
  else
    log_warn "Anolisa 目录不存在: $AGENTICOS_BASE"
  fi

  return $failed
}

# ============================================================
# [Section 4] 模块注册表
# ============================================================

# --- 存储结构 ---
MODULES=()
declare -A MODULE_DESC MODULE_CATEGORY MODULE_LOCAL_PATH MODULE_REMOTE_URL MODULE_DEPENDS MODULE_RPM MODULE_RESTART_GATEWAY

# --- 注册函数 ---
register_module() {
  local name="$1"; shift
  MODULES+=("$name")

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --desc)         shift; MODULE_DESC["$name"]="$1" ;;
      --category)     shift; MODULE_CATEGORY["$name"]="$1" ;;
      --local-path)   shift; MODULE_LOCAL_PATH["$name"]="$1" ;;
      --remote-url)   shift; MODULE_REMOTE_URL["$name"]="$1" ;;
      --depends)      shift; MODULE_DEPENDS["$name"]="$1" ;;
      --rpm)          shift; MODULE_RPM["$name"]="$1" ;;
      --restart-gateway) shift; MODULE_RESTART_GATEWAY["$name"]="$1" ;;
    esac
    shift
  done
}

# --- 模块注册（按依赖顺序） ---

register_module "openclaw" \
  --desc "OpenClaw 本体（Agent Gateway）" \
  --category "core" \
  --local-path "" \
  --remote-url "" \
  --depends "" \
  --rpm ""

register_module "osbase" \
  --desc "系统内存自动优化（小规格实例自动调优）" \
  --category "system" \
  --local-path "" \
  --remote-url "" \
  --depends "" \
  --rpm "alinux-base-setup-swas"

register_module "skill" \
  --desc "预置技能库（网络诊断/日志分析/部署辅助等）" \
  --category "skill" \
  --local-path "$AGENTICOS_BASE/skills" \
  --remote-url "" \
  --depends "openclaw" \
  --rpm ""

register_module "skillfs" \
  --desc "精简 Skill 文件系统（降低 Token 开销）" \
  --category "skill" \
  --local-path "$RUNTIME_SKILLS_DIR/skillfs" \
  --remote-url "" \
  --depends "openclaw" \
  --rpm "skillfs"

register_module "ws-ckpt" \
  --desc "会话快照与恢复" \
  --category "skill" \
  --local-path "$RUNTIME_SKILLS_DIR/ws-ckpt" \
  --remote-url "" \
  --depends "openclaw" \
  --rpm "ws-ckpt"

register_module "sec-core" \
  --desc "安全运行时防护（sandbox + asset-verify + skill）" \
  --category "hybrid" \
  --local-path "$AGENTICOS_BASE/skills/agent-sec-core" \
  --remote-url "" \
  --depends "openclaw" \
  --rpm "agent-sec-core" \
  --restart-gateway "true"

register_module "sight" \
  --desc "可观测性（eBPF Agent 行为监控 + Token 统计）" \
  --category "system" \
  --local-path "" \
  --remote-url "" \
  --depends "openclaw" \
  --rpm "agentsight"

register_module "token-less" \
  --desc "Token 节约（上下文压缩 + 输出过滤 + RTK 命令重写）" \
  --category "plugin" \
  --local-path "/usr/share/tokenless/adapters/openclaw" \
  --remote-url "" \
  --depends "openclaw" \
  --rpm "tokenless" \
  --restart-gateway "true"

register_module "skvm" \
  --desc "skvm skill bank" \
  --category "system" \
  --local-path "/usr/share/anolisa/runtime/skvm" \
  --remote-url "" \
  --depends "openclaw" \
  --rpm "skvm-skills-openclaw"

refresh_module_paths() {
  if [[ -z "$RUNTIME_SKILLS_DIR_USER_SET" ]]; then
    RUNTIME_SKILLS_DIR="$AGENTICOS_BASE/runtime/skills"
  fi

  MODULE_LOCAL_PATH[skill]="$AGENTICOS_BASE/skills"
  MODULE_LOCAL_PATH[skillfs]="$RUNTIME_SKILLS_DIR/skillfs"
  MODULE_LOCAL_PATH[ws-ckpt]="$RUNTIME_SKILLS_DIR/ws-ckpt"
  MODULE_LOCAL_PATH[sec-core]="$AGENTICOS_BASE/skills/agent-sec-core"
  MODULE_LOCAL_PATH[skvm]="$AGENTICOS_BASE/runtime/skvm"
}

# --- 模式存储结构 ---
MODES=()
declare -A MODE_DESC MODE_MODULES MODE_HINT

# --- 模式注册函数 ---
register_mode() {
  local name="$1"; shift
  MODES+=("$name")

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --desc)    shift; MODE_DESC["$name"]="$1" ;;
      --modules) shift; MODE_MODULES["$name"]="$1" ;;
      --hint)    shift; MODE_HINT["$name"]="$1" ;;
    esac
    shift
  done
}

# --- 模式注册（按菜单顺序） ---

register_mode "recommended" \
  --desc "推荐安装 — 安全 + 提速 + 省钱，适合大多数用户" \
  --modules "osbase skill skillfs ws-ckpt skvm sec-core token-less" \
  --hint "覆盖安全防护、运行提速、Token 节约"

register_mode "all" \
  --desc "完整安装 — 所有 AgenticOS 能力" \
  --modules "__ALL__" \
  --hint "在推荐基础上额外包含可观测性等组件"

register_mode "custom" \
  --desc "自定义安装 — 逐个选择模块" \
  --modules "__CUSTOM__" \
  --hint "适合了解各组件的高级用户"

# --- 模式解析函数 ---
# 根据模式 key 填充 SELECTED_MODULES 数组
# __ALL__    -> 所有模块（排除 openclaw，它由独立检测逻辑处理）
# __CUSTOM__ -> 不做任何操作（调用方负责交互选择）
# 其他       -> 按空格分割 MODE_MODULES[key]
resolve_mode_modules() {
  local mode_key="$1"
  local modules_str="${MODE_MODULES[$mode_key]:-}"

  if [[ -z "$modules_str" ]]; then
    die "未知的安装模式: $mode_key"
  fi

  if [[ "$modules_str" == "__ALL__" ]]; then
    for name in "${MODULES[@]}"; do
      [[ "$name" == "openclaw" ]] && continue
      SELECTED_MODULES+=("$name")
    done
  elif [[ "$modules_str" == "__CUSTOM__" ]]; then
    : # 不做操作，由调用方处理交互选择
  else
    for name in $modules_str; do
      SELECTED_MODULES+=("$name")
    done
  fi
}

# ============================================================
# [Section 5] 模块实现函数
# ============================================================

# ====================== openclaw ========================

detect_openclaw() {
  command -v openclaw &>/dev/null
}

install_openclaw() {
  if detect_openclaw; then
    log_skip "OpenClaw 已安装: $(openclaw --version 2>/dev/null || echo 'unknown version')"
    return 0
  fi

  log_info "正在安装 OpenClaw..."
  run_cmd npm install -g openclaw || return 1
  log_info "OpenClaw 安装完成: $(openclaw --version)"
}

cleanup_openclaw() {
  if command -v openclaw &>/dev/null; then
    run_cmd npm uninstall -g openclaw
    log_info "OpenClaw 已卸载"
  fi
}

verify_openclaw() {
  detect_openclaw || { log_error "openclaw command not found"; return 1; }
  log_info "OpenClaw 验证通过: $(openclaw --version 2>/dev/null)"
}

# ====================== osbase ========================

detect_osbase() {
  rpm -q "${MODULE_RPM[osbase]}" &>/dev/null
}

install_osbase() {
  if run_cmd rpm -q "${MODULE_RPM[osbase]}" &>/dev/null; then
    log_info "osbase RPM 已安装 (${MODULE_RPM[osbase]})"

    # 检查内存大小，报告优化状态
    local mem_total_mb
    mem_total_mb="$(free -m 2>/dev/null | awk '/^Mem:/ {print $2}')"
    if [[ -n "$mem_total_mb" ]] && [[ "$mem_total_mb" -lt 4096 ]]; then
      log_info "内存 ${mem_total_mb}MB < 4GB，小规格实例优化已自动生效"
    else
      log_info "内存 ${mem_total_mb:-?}MB >= 4GB，无需小规格优化"
    fi
    return 0
  fi

  # 未安装：尝试自动安装或提示
  if [[ "${AUTO_INSTALL_RPM:-0}" -eq 1 ]]; then
    log_info "正在安装 RPM 依赖: ${MODULE_RPM[osbase]} ..."
    if run_cmd dnf install -y "${MODULE_RPM[osbase]}" &>/dev/null; then
      log_info "${MODULE_RPM[osbase]} 安装成功"
      return 0
    else
      log_warn "${MODULE_RPM[osbase]} 安装失败"
    fi
  fi

  # osbase 默认就启用了，如果没有安装说明不是 Anolis OS SWAS 环境
  log_warn "osbase (${MODULE_RPM[osbase]}) 未安装"
  log_warn "此包通常预装于 Anolis OS SWAS 实例，无需手动安装"
  return 0  # 不视为失败
}

cleanup_osbase() {
  # osbase 是系统预装的，不应卸载
  log_warn "osbase 是系统预装包，跳过卸载"
}

verify_osbase() {
  if detect_osbase; then
    log_info "osbase 已就绪 (${MODULE_RPM[osbase]})"
    return 0
  fi
  log_warn "osbase 未安装（非 SWAS 环境，可忽略）"
  return 0
}

# ====================== skill ========================

detect_skill() {
  local path="${MODULE_LOCAL_PATH[skill]}"
  [[ -d "$path" ]] && find "$path" -maxdepth 2 -name "SKILL.md" -print -quit 2>/dev/null | grep -q .
}

install_skill() {
  local src="${MODULE_LOCAL_PATH[skill]}"

  if ! [[ -d "$src" ]]; then
    log_error "Skill 源目录不存在: $src"
    return 1
  fi

  log_info "正在迁移 SKILL 技能库..."
  migrate_skills "$src" "$OPENCLAW_SKILLS_DIR" "${SKILL_SELECT_MODE:-0}" || return 1
}

cleanup_skill() {
  local src="${MODULE_LOCAL_PATH[skill]}"
  unmigrate_skills "$src" "$OPENCLAW_SKILLS_DIR"
  log_info "SKILL 技能库已清理"
}

verify_skill() {
  local src="${MODULE_LOCAL_PATH[skill]}"
  local count
  count="$(find "$OPENCLAW_SKILLS_DIR" -maxdepth 2 -name "SKILL.md" 2>/dev/null | wc -l)"
  if [[ "$count" -gt 0 ]]; then
    log_info "SKILL 验证通过 ($count 个技能已就绪)"
    return 0
  fi
  log_error "No skills found in $OPENCLAW_SKILLS_DIR"
  return 1
}

# ====================== skillfs ========================

detect_skillfs() {
  rpm -q "${MODULE_RPM[skillfs]}" &>/dev/null
}

install_skillfs() {
  ensure_rpm "${MODULE_RPM[skillfs]}" || return 1

  local src="${MODULE_LOCAL_PATH[skillfs]}"
  if [[ -d "$src" ]]; then
    log_info "正在迁移 skillfs Skills..."
    migrate_skills "$src" "$OPENCLAW_SKILLS_DIR" || return 1
  else
    log_error "skillfs Skill 目录不存在: $src"
    log_error "请确认 ${MODULE_RPM[skillfs]} RPM 已正确安装"
    return 1
  fi
}

cleanup_skillfs() {
  local src="${MODULE_LOCAL_PATH[skillfs]}"
  unmigrate_skills "$src" "$OPENCLAW_SKILLS_DIR"
  log_info "skillfs 已清理"
}

verify_skillfs() {
  detect_skillfs || { log_error "${MODULE_RPM[skillfs]} RPM 未安装"; return 1; }
  local src="${MODULE_LOCAL_PATH[skillfs]}"
  [[ -d "$src" ]] || { log_error "skillfs 源目录不存在: $src"; return 1; }
  local count
  count="$(find "$OPENCLAW_SKILLS_DIR" -maxdepth 1 -name 'skillfs' -type d 2>/dev/null | wc -l)"
  [[ "$count" -gt 0 ]] || { log_error "skillfs Skill 未迁移到 $OPENCLAW_SKILLS_DIR"; return 1; }
  log_info "skillfs 验证通过"
}

# ====================== ws-ckpt ========================

detect_ws_ckpt() {
  rpm -q "${MODULE_RPM[ws-ckpt]}" &>/dev/null
}

install_ws_ckpt() {
  ensure_rpm "${MODULE_RPM[ws-ckpt]}" || return 1

  local src="${MODULE_LOCAL_PATH[ws-ckpt]}"
  if [[ -d "$src" ]]; then
    log_info "正在迁移 ws-ckpt Skills..."
    migrate_skills "$src" "$OPENCLAW_SKILLS_DIR" || return 1
  else
    log_error "ws-ckpt Skill 目录不存在: $src"
    log_error "请确认 ${MODULE_RPM[ws-ckpt]} RPM 已正确安装"
    return 1
  fi
}

cleanup_ws_ckpt() {
  local src="${MODULE_LOCAL_PATH[ws-ckpt]}"
  unmigrate_skills "$src" "$OPENCLAW_SKILLS_DIR"
  log_info "ws-ckpt 已清理"
}

verify_ws_ckpt() {
  detect_ws_ckpt || { log_error "${MODULE_RPM[ws-ckpt]} RPM 未安装"; return 1; }
  local src="${MODULE_LOCAL_PATH[ws-ckpt]}"
  [[ -d "$src" ]] || { log_error "ws-ckpt 源目录不存在: $src"; return 1; }
  local count
  count="$(find "$OPENCLAW_SKILLS_DIR" -maxdepth 1 -name 'ws-ckpt' -type d 2>/dev/null | wc -l)"
  [[ "$count" -gt 0 ]] || { log_error "ws-ckpt Skill 未迁移到 $OPENCLAW_SKILLS_DIR"; return 1; }
  log_info "ws-ckpt 验证通过"
}

# ====================== sec-core ========================

detect_sec_core() {
  # 以 deploy.sh 是否存在为主检测，其次判断 RPM
  [[ -f "$SEC_CORE_DEPLOY_SH" ]] || rpm -q "${MODULE_RPM[sec-core]}" &>/dev/null
}

install_sec_core() {
  # Step 1: 检查/安装 agent-sec-core RPM
  ensure_rpm "${MODULE_RPM[sec-core]}" || return 1
  log_info "${MODULE_RPM[sec-core]} RPM 已安装"

  # Step 2: 执行 OpenClaw 集成部署脚本
  if [[ -x "$SEC_CORE_DEPLOY_SH" ]]; then
    log_info "正在执行 sec-core OpenClaw 集成部署脚本..."
    run_cmd bash "$SEC_CORE_DEPLOY_SH" || {
      log_error "deploy.sh 执行失败: $SEC_CORE_DEPLOY_SH"
      return 1
    }
    log_info "sec-core 部署脚本执行完成"
  else
    log_warn "sec-core 部署脚本未找到: $SEC_CORE_DEPLOY_SH"
    log_warn "跳过自动部署，仅进行 Skill 迁移"
  fi

  # Step 3: 迁移 agent-sec-core Skill
  local src="${MODULE_LOCAL_PATH[sec-core]}"
  if [[ -d "$src" ]]; then
    log_info "正在迁移 sec-core Skills..."
    migrate_skills "$src" "$OPENCLAW_SKILLS_DIR" || return 1
  else
    log_error "sec-core Skill 目录不存在: $src"
    log_error "请确认 ${MODULE_RPM[sec-core]} RPM 已正确安装"
    return 1
  fi

  # Step 4: 验证关键二进制
  if [[ -x /usr/local/bin/linux-sandbox ]]; then
    log_info "linux-sandbox: /usr/local/bin/linux-sandbox 已就绪"
  else
    log_warn "linux-sandbox 未找到，部分安全功能可能平降级"
  fi
}

cleanup_sec_core() {
  local src="${MODULE_LOCAL_PATH[sec-core]}"
  local ext_dir="$OPENCLAW_HOME/extensions/agent-sec"

  unmigrate_skills "$src" "$OPENCLAW_SKILLS_DIR"
  log_info "sec-core Skill 已清理"

  if command -v openclaw &>/dev/null; then
    run_cmd openclaw plugins uninstall --force agent-sec || {
      log_warn "agent-sec 插件卸载命令失败，继续执行本地清理"
    }
  fi

  run_cmd rm -rf "$ext_dir"
  remove_plugin_config_entries "agent-sec" "$ext_dir"
  refresh_openclaw_plugin_registry || true
}

verify_sec_core() {
  rpm -q "${MODULE_RPM[sec-core]}" &>/dev/null || { log_error "${MODULE_RPM[sec-core]} RPM 未安装"; return 1; }
  [[ -x /usr/local/bin/linux-sandbox ]] || log_warn "linux-sandbox 二进制未找到"
  local count
  count="$(find "$OPENCLAW_SKILLS_DIR" -maxdepth 1 -name 'agent-sec-core' -type d 2>/dev/null | wc -l)"
  [[ "$count" -gt 0 ]] || { log_error "sec-core Skill 未迁移到 $OPENCLAW_SKILLS_DIR"; return 1; }
  log_info "sec-core 验证通过（RPM + deploy.sh + Skill 均已就绪）"
}

# ====================== sight ========================

detect_sight() {
  rpm -q "${MODULE_RPM[sight]}" &>/dev/null
}

install_sight() {
  ensure_rpm "${MODULE_RPM[sight]}" || return 1

  # agentsight 没有 Skill 文件，只有二进制 + systemd 服务
  log_info "agentsight 已安装，检查运行状态..."

  # 检查二进制
  if [[ -x /usr/local/bin/agentsight ]]; then
    log_info "agentsight 二进制: /usr/local/bin/agentsight"
  else
    log_warn "agentsight 二进制未找到 (/usr/local/bin/agentsight)"
  fi

  # 检查并可选启动 systemd 服务
  if command -v systemctl &>/dev/null; then
    local svc_status
    svc_status="$(systemctl is-enabled agentsight.service 2>/dev/null || echo 'not-found')"
    if [[ "$svc_status" == "not-found" ]]; then
      log_warn "agentsight.service 未找到，请确认 RPM 安装正确"
    elif [[ "$svc_status" == "disabled" ]]; then
      log_info "agentsight.service 已安装但未启用"
      log_info "如需启用: systemctl enable --now agentsight.service"
    else
      log_info "agentsight.service 状态: $svc_status"
    fi
  fi

  log_info "sight (agentsight) 配置完成"
}

cleanup_sight() {
  # agentsight 由 systemd 管理，此处只停止服务，不卸载 RPM
  log_info "sight 已清理（RPM 保留）"
}

verify_sight() {
  detect_sight || { log_error "${MODULE_RPM[sight]} RPM 未安装"; return 1; }
  [[ -x /usr/local/bin/agentsight ]] || { log_error "agentsight 二进制未找到"; return 1; }
  log_info "sight 验证通过 (agentsight 已安装)"
}

# ====================== token-less ========================

detect_token_less() {
  # 检查 RPM 或 OpenClaw plugin 目录
  rpm -q "${MODULE_RPM[token-less]}" &>/dev/null || \
    [[ -f "${MODULE_LOCAL_PATH[token-less]}/openclaw.plugin.json" ]]
}

install_token_less() {
  ensure_rpm "${MODULE_RPM[token-less]}" || return 1

  # 安装脚本存在则调用 --openclaw（专门处理 OpenClaw plugin 配置 + cosh hooks）
  # RPM %post 通常已自动执行 --install，这里用 --openclaw 做幂等性重运行
  local install_sh="/usr/share/tokenless/scripts/install.sh"
  if [[ -x "$install_sh" ]]; then
    log_info "正在执行 tokenless OpenClaw 集成配置..."
    run_cmd "$install_sh" --openclaw || {
      log_warn "tokenless install.sh --openclaw 返回非零，请检查 OpenClaw 配置是否正确"
    }
    log_info "tokenless OpenClaw 配置完成"
  else
    log_warn "配置脚本未找到: $install_sh"
    log_warn "RPM %post 应已自动执行配置，如有异常请重装 ${MODULE_RPM[token-less]}"
  fi

  # 验证 OpenClaw plugin 目录
  local plugin_dir="${MODULE_LOCAL_PATH[token-less]}"
  if [[ -f "$plugin_dir/openclaw.plugin.json" ]]; then
    log_info "OpenClaw plugin 已就绪: $plugin_dir"
    json_init "$OPENCLAW_CONFIG"
    backup_file "$OPENCLAW_CONFIG" >/dev/null
    json_write_with_args "$OPENCLAW_CONFIG" \
      '.plugins.enabled = true | .plugins.allow = (.plugins.allow // [] | map(select(. != "plugin")) | unique)'
    log_info "已清理 plugins.allow 中无效的 plugin 项"

    if jq -e '.plugins.entries["tokenless-openclaw"]? != null' "$OPENCLAW_CONFIG" >/dev/null 2>&1; then
      log_info "检测到 tokenless-openclaw 已通过配置注册，跳过 plugins.load.paths 追加以避免重复插件 ID"
    else
      # 仅在尚未通过配置注册时追加全局加载路径，避免与 config plugin 重复
      json_write_with_args "$OPENCLAW_CONFIG" --arg plugin_dir "$plugin_dir" \
        '.plugins.enabled = true | .plugins.load.paths = ((.plugins.load.paths // []) + [$plugin_dir] | unique)'
      log_info "OpenClaw 已添加 token-less plugin 路径: $plugin_dir"
    fi
  else
    log_warn "OpenClaw plugin 目录未找到: $plugin_dir/openclaw.plugin.json"
  fi

  log_info "token-less 配置完成"
}

cleanup_token_less() {
  local plugin_dir="${MODULE_LOCAL_PATH[token-less]}"
  local ext_dir="$OPENCLAW_HOME/extensions/tokenless"

  # 调用卸载脚本清理 hooks
  local install_sh="/usr/share/tokenless/scripts/install.sh"
  if [[ -x "$install_sh" ]]; then
    run_cmd "$install_sh" --uninstall-openclaw 2>/dev/null || true
    log_info "tokenless hooks 已清理"
  fi

  run_cmd rm -rf "$ext_dir"
  remove_plugin_config_entries "tokenless-openclaw" "$plugin_dir"
  log_info "token-less OpenClaw 配置已清理"
  refresh_openclaw_plugin_registry || true
  log_info "token-less 已清理"
}

verify_token_less() {
  rpm -q "${MODULE_RPM[token-less]}" &>/dev/null || { log_error "${MODULE_RPM[token-less]} RPM 未安装"; return 1; }
  command -v tokenless &>/dev/null || { log_error "tokenless 命令未找到"; return 1; }
  command -v rtk &>/dev/null || log_warn "rtk 命令未找到 (/usr/bin/rtk)"
  [[ -f "${MODULE_LOCAL_PATH[token-less]}/openclaw.plugin.json" ]] || \
    log_warn "OpenClaw plugin 文件未找到: ${MODULE_LOCAL_PATH[token-less]}/openclaw.plugin.json"
  log_info "token-less 验证通过"
}

# ====================== skvm ========================

detect_skvm() {
  rpm -q "${MODULE_RPM[skvm]}" &>/dev/null
}

install_skvm() {
  ensure_rpm "${MODULE_RPM[skvm]}" || return 1

  log_info "skvm 已安装，检查运行状态..."

  # 检查并可选启动 systemd 服务
  if command -v systemctl &>/dev/null; then
    local svc_status
    svc_status="$(systemctl --user is-enabled skvm-bridged.service 2>/dev/null | tr -d '[:space:]')"
    [[ -z "$svc_status" ]] && svc_status="not-found"
    if [[ "$svc_status" == "not-found" ]]; then
      log_warn "skvm-bridged.service 未找到，请确认 RPM 安装正确"
    elif [[ "$svc_status" == "disabled" ]]; then
      log_info "skvm-bridged.service 已安装但未启用，正在启动..."
      run_cmd systemctl --user enable --now skvm-bridged.service
      log_info "skvm-bridged.service 已启动"
    else
      log_info "skvm-bridged.service 状态: $svc_status"
    fi
  fi

  log_info "skvm 配置完成"
}

cleanup_skvm() {
  run_cmd systemctl --user stop skvm-bridged.service 2>/dev/null || true
  run_cmd systemctl --user disable skvm-bridged.service 2>/dev/null || true
  cleanup_skvm_skill_links
}

verify_skvm() {
  rpm -q "${MODULE_RPM[skvm]}" &>/dev/null || { log_error "${MODULE_RPM[skvm]} RPM 未安装"; return 1; }

  if command -v systemctl &>/dev/null; then
    local svc_status
    svc_status="$(systemctl --user is-active skvm-bridged.service 2>/dev/null | tr -d '[:space:]')"
    [[ -z "$svc_status" ]] && svc_status="inactive"
    if [[ "$svc_status" == "active" ]]; then
      log_info "skvm-bridged.service 运行中"
    else
      log_warn "skvm-bridged.service 未运行 (状态: $svc_status)"
    fi
  fi

  log_info "skvm 验证通过"
}


# ============================================================
# [Section 6] 模块调度引擎
# ============================================================

install_label() {
  case "${MODULE_CATEGORY[$1]}" in
    core)    echo "本体" ;;
    system)  echo "系统层, 检查状态" ;;
    skill)   echo "SKILL, cp 迁移" ;;
    plugin)  echo "Plugin, 脚本/安装" ;;
    hybrid)  echo "Skill+Plugin, 迁移" ;;
    *)       echo "${MODULE_CATEGORY[$1]}" ;;
  esac
}

run_module_install() {
  local name="$1"
  local fn_name="${name//-/_}"
  local prev_module="${CURRENT_MODULE:-}"

  if [[ "${DRY_RUN:-0}" == "1" ]]; then
    print_divider
    log_info "$name — 将执行安装 [dry-run]"
    print_divider
    return 0
  fi

  print_divider
  CURRENT_MODULE="$name"
  log_info "$name — 正在安装..."
  if "install_${fn_name}"; then
    state_set_module "$name" "installed"
    log_info "$name — 安装成功"
    CURRENT_MODULE="$prev_module"
    print_divider
    return 0
  else
    local err_msg="install_${fn_name} returned non-zero"
    state_set_module "$name" "failed" "$err_msg"
    log_error "$name — 安装失败"
    CURRENT_MODULE="$prev_module"
    print_divider
    return 1
  fi
}

run_module_cleanup() {
  local name="$1"
  local fn_name="${name//-/_}"
  local prev_module="${CURRENT_MODULE:-}"

  if [[ "${DRY_RUN:-0}" == "1" ]]; then
    print_divider
    log_info "$name — 将执行清理 [dry-run]"
    print_divider
    return 0
  fi

  print_divider
  CURRENT_MODULE="$name"
  if "cleanup_${fn_name}"; then
    state_set_module "$name" "cleaned"
    log_info "$name — 已清理"
    CURRENT_MODULE="$prev_module"
    print_divider
    return 0
  fi

  state_set_module "$name" "cleanup-failed" "cleanup_${fn_name} returned non-zero"
  log_error "$name — 清理失败"
  CURRENT_MODULE="$prev_module"
  print_divider
  return 1
}

run_module_verify() {
  local name="$1"
  local fn_name="${name//-/_}"

  if "verify_${fn_name}"; then
    log_info "$name — 验证通过"
    return 0
  else
    log_error "$name — 验证失败"
    return 1
  fi
}

run_install_batch() {
  local -a selected=("$@")
  local failed=0

  # 自动补上 openclaw 依赖
  local has_oc_dep=0
  for m in "${selected[@]}"; do
    if [[ "$m" != "openclaw" ]] && [[ "${MODULE_DEPENDS[$m]}" == *"openclaw"* ]]; then
      has_oc_dep=1
      break
    fi
  done
  if [[ "$has_oc_dep" -eq 1 ]] && ! detect_openclaw; then
    if [[ " ${selected[*]} " != *" openclaw "* ]]; then
      selected=("openclaw" "${selected[@]}")
      log_warn "所选模块依赖 OpenClaw，已自动加入安装队列"
    fi
  fi

  for name in "${selected[@]}"; do
    run_module_install "$name" || ((failed++))
  done

  return $failed
}

# ============================================================
# [Section 7] 交互模式
# ============================================================

# --- 安装模式菜单 ---
# 输出: 通过 echo 返回用户选择的模式 key
display_mode_menu() {
  echo "" >&2
  local i=1
  for mode_key in "${MODES[@]}"; do
    local desc="${MODE_DESC[$mode_key]}"
    local hint="${MODE_HINT[$mode_key]}"
    local modules_str="${MODE_MODULES[$mode_key]}"

    printf "    %d) %s\n" "$i" "$desc" >&2
    printf "       ${CYAN}%s${NC}\n" "$hint" >&2

    # 显示包含的模块列表（非 custom 模式）
    if [[ "$modules_str" == "__ALL__" ]]; then
      local count=0
      for name in "${MODULES[@]}"; do
        [[ "$name" == "openclaw" ]] && continue
        count=$((count + 1))
      done
      printf "       包含: 全部 %d 个模块\n" "$count" >&2
    elif [[ "$modules_str" != "__CUSTOM__" ]]; then
      printf "       包含: %s\n" "$modules_str" >&2
    fi
    echo "" >&2
    i=$((i + 1))
  done

  local choice
  choice="$(prompt_choice "  请选择 [1-${#MODES[@]}] (默认: 1):" "1")"

  # 验证输入范围
  if ! [[ "$choice" =~ ^[0-9]+$ ]] || [[ "$choice" -lt 1 ]] || [[ "$choice" -gt ${#MODES[@]} ]]; then
    log_warn "无效输入，使用默认推荐模式" >&2
    choice=1
  fi

  local idx=$((choice - 1))
  echo "${MODES[$idx]}"
}

# --- 模块交互式选择（仅自定义模式使用）---
# 从原 interactive_mode() 提取，逻辑不变
# 输出: 追加到 SELECTED_MODULES 全局数组
module_select_interactive() {
  log_step "选择要集成的 AgenticOS 能力"
  echo "  输入编号，空格分隔（例如: 1 3 5），a 全选，0 取消全选"
  echo ""

  local i=1
  local -a menu_map=()
  for name in "${MODULES[@]}"; do
    [[ "$name" == "openclaw" ]] && continue
    local desc="${MODULE_DESC[$name]}"
    local cat
    cat="$(install_label "$name")"
    local installed=""
    detect_"${name//-/_}" &>/dev/null && installed=" [已安装]"
    printf "    %2d) %-12s — %s%s (%s)\n" "$i" "$name" "$desc" "$installed" "$cat"
    menu_map["$i"]="$name"
    i=$((i + 1))
  done

  echo ""
  local choices
  choices="$(prompt_choice "  请选择 [编号/a/0]:")"

  if [[ "$choices" == "a" || "$choices" == "A" ]]; then
    for name in "${MODULES[@]}"; do
      [[ "$name" == "openclaw" ]] && continue
      SELECTED_MODULES+=("$name")
    done
  else
    for num in $choices; do
      local m="${menu_map[$num]:-}"
      [[ -n "$m" ]] && SELECTED_MODULES+=("$m")
    done
  fi
}

# --- 展示安装计划 ---
# 用法: show_install_plan "mode_desc" module1 module2 ...
show_module_command_preview() {
  local name="$1"

  case "$name" in
    openclaw)
      echo "      - 检测: command -v openclaw"
      echo "      - 安装: npm install -g openclaw"
      ;;
    osbase)
      echo "      - 检测: rpm -q ${MODULE_RPM[osbase]}"
      echo "      - 按需安装: dnf install -y ${MODULE_RPM[osbase]}  (仅 --install-deps)"
      ;;
    skill)
      echo "      - 迁移: cp -r <skill_dir> \"$OPENCLAW_SKILLS_DIR/<skill_name>\""
      echo "      - 来源: ${MODULE_LOCAL_PATH[skill]} -> $OPENCLAW_SKILLS_DIR"
      ;;
    skillfs)
      echo "      - 检测: rpm -q ${MODULE_RPM[skillfs]}"
      echo "      - 按需安装: dnf install -y ${MODULE_RPM[skillfs]}  (仅 --install-deps)"
      echo "      - 迁移: cp -r ${MODULE_LOCAL_PATH[skillfs]} <...> -> $OPENCLAW_SKILLS_DIR"
      ;;
    ws-ckpt)
      echo "      - 检测: rpm -q ${MODULE_RPM[ws-ckpt]}"
      echo "      - 按需安装: dnf install -y ${MODULE_RPM[ws-ckpt]}  (仅 --install-deps)"
      echo "      - 迁移: cp -r ${MODULE_LOCAL_PATH[ws-ckpt]} <...> -> $OPENCLAW_SKILLS_DIR"
      ;;
    skvm)
      echo "      - 检测: rpm -q ${MODULE_RPM[skvm]}"
      echo "      - 按需安装: dnf install -y ${MODULE_RPM[skvm]}  (仅 --install-deps)"
      echo "      - 服务: systemctl --user enable --now skvm-bridged.service"
      ;;
    sec-core)
      echo "      - 检测: rpm -q ${MODULE_RPM[sec-core]}"
      echo "      - 按需安装: dnf install -y ${MODULE_RPM[sec-core]}  (仅 --install-deps)"
      echo "      - 部署: bash $SEC_CORE_DEPLOY_SH"
      echo "      - 迁移: cp -r ${MODULE_LOCAL_PATH[sec-core]} <...> -> $OPENCLAW_SKILLS_DIR"
      ;;
    sight)
      echo "      - 检测: rpm -q ${MODULE_RPM[sight]}"
      echo "      - 按需安装: dnf install -y ${MODULE_RPM[sight]}  (仅 --install-deps)"
      echo "      - 可选启用: systemctl enable --now agentsight.service"
      ;;
    token-less)
      echo "      - 检测: rpm -q ${MODULE_RPM[token-less]}"
      echo "      - 按需安装: dnf install -y ${MODULE_RPM[token-less]}  (仅 --install-deps)"
      echo "      - 集成: /usr/share/tokenless/scripts/install.sh --openclaw"
      echo "      - 配置: jq 写入 $OPENCLAW_CONFIG (plugins.allow/plugins.load.paths)"
      ;;
    *)
      echo "      - 执行: install_${name//-/_}"
      ;;
  esac
}

show_install_plan() {
  local mode_desc="$1"; shift
  local -a plan_modules=("$@")

  log_step "安装计划"
  if [[ -n "$mode_desc" ]]; then
    echo "  模式: $mode_desc"
    echo ""
  fi
  for name in "${plan_modules[@]}"; do
    if [[ "$name" == "skill" && ${#PRESELECTED_SKILL_PATHS[@]} -gt 0 ]]; then
      echo "    ✓ skill — 已选 ${#PRESELECTED_SKILL_PATHS[@]} 个 Skills"
    elif [[ "$name" == "skill" ]]; then
      echo "    ✓ skill — 全部 Skills"
    else
      echo "    ✓ $name ($(install_label "$name"))"
    fi
    show_module_command_preview "$name"
    print_divider
  done
  echo ""
}

interactive_mode() {
  echo ""
  echo -e "${BOLD}╔═══════════════════════════════════════════════════════╗${NC}"
  echo -e "${BOLD}║     Agentic OS for OpenClaw 安装向导 v${SCRIPT_VERSION}     ║${NC}"
  echo -e "${BOLD}╚═══════════════════════════════════════════════════════╝${NC}"

  prerequisites_check || die "前置检测未通过，请修复后重试"

  # OpenClaw 检测（保持不变）
  log_step "OpenClaw 检测"
  if detect_openclaw; then
    log_info "OpenClaw 已安装: $(openclaw --version 2>/dev/null || echo 'unknown')"
  else
    echo ""
    echo "  OpenClaw 未安装，请选择："
    echo "    1) 自动安装 (npm install -g openclaw)"
    echo "    2) 跳过（我稍后自行安装）"
    local oc_choice
    oc_choice="$(prompt_choice "  请选择 [1-2]:" "1")"
    case "$oc_choice" in
      1) SELECTED_MODULES+=("openclaw") ;;
      2) log_warn "跳过 OpenClaw 安装，部分模块可能无法配置" ;;
    esac
  fi

  # 安装模式选择
  log_step "选择安装模式"

  # 非 TTY 环境默认使用推荐模式
  local mode_key
  if [[ ! -t 0 ]]; then
    log_info "非交互环境，使用推荐模式"
    mode_key="recommended"
  else
    mode_key="$(display_mode_menu)"
  fi

  # 根据模式解析模块
  PRESELECTED_SKILL_PATHS=()
  SKILL_SELECT_MODE=0

  if [[ "$mode_key" == "custom" ]]; then
    # 自定义模式: 进入模块多选（追加到 SELECTED_MODULES）
    module_select_interactive

    # 如果选了 skill 模块，进入 skill 子选择
    if [[ " ${SELECTED_MODULES[*]} " == *" skill "* ]]; then
      SKILL_SELECT_MODE=1
      local skill_src="${MODULE_LOCAL_PATH[skill]}"
      if [[ -d "$skill_src" ]]; then
        local -a _all_skills=()
        if [[ -f "$skill_src/SKILL.md" ]]; then
          _all_skills+=("$skill_src")
        else
          for _sp in "$skill_src"/*/; do
            [[ -d "$_sp" && -f "$_sp/SKILL.md" ]] && _all_skills+=("$_sp")
          done
        fi
        if [[ ${#_all_skills[@]} -gt 1 ]]; then
          log_step "选择要安装的 Skills"
          select_skills_interactive "${_all_skills[@]}"
          PRESELECTED_SKILL_PATHS=("${SELECTED_SKILL_PATHS[@]}")
        fi
      fi
    fi
  else
    # 非自定义模式: 按模式解析模块，skill 全选
    resolve_mode_modules "$mode_key"
  fi

  local -a selected=("${SELECTED_MODULES[@]}")

  if [[ ${#selected[@]} -eq 0 ]]; then
    log_warn "未选择任何模块"
    exit 0
  fi

  # 展示安装计划
  show_install_plan "${MODE_DESC[$mode_key]:-}" "${selected[@]}"

  # 初始化
  json_init "$OPENCLAW_CONFIG"
  state_init
  backup_file "$OPENCLAW_CONFIG" >/dev/null

  # 执行
  log_step "执行安装"
  local failed=0
  run_install_batch "${selected[@]}" || failed=$?
  normalize_openclaw_plugin_config

  # 汇总
  log_step "安装报告"
  local ok=0 skip=0 fail=0
  for name in "${selected[@]}"; do
    local st
    st="$(state_get_module "$name")"
    local status_text
    case "$st" in
      installed) status_text="installed"; ok=$((ok + 1)) ;;
      failed)    status_text="failed"; fail=$((fail + 1)) ;;
      *)         status_text="skipped"; skip=$((skip + 1)) ;;
    esac

    echo "  - 组件: $name  状态: $status_text"
    if [[ -n "${MODULE_EXEC_CMDS[$name]:-}" ]]; then
      echo "    执行命令:"
      while IFS= read -r cmd_line; do
        [[ -n "$cmd_line" ]] && echo "      * $cmd_line"
      done <<< "${MODULE_EXEC_CMDS[$name]}"
    else
      echo "    执行命令: (无外部 shell 命令或未触发)"
    fi
    print_divider
  done

  echo ""
  echo -e "  成功: ${GREEN}${ok}${NC}  失败: ${RED}${fail}${NC}  跳过: ${CYAN}${skip}${NC}"
  if [[ "$fail" -gt 0 ]]; then
    echo -e "  ${YELLOW}失败模块可通过 --retry 重试，或 --rollback 回滚${NC}"
  fi
  echo -e "  配置备份: ${BACKUP_DIR}/"

  # 检查是否有模块需要重启 OpenClaw gateway（保留询问）
  local need_restart=0
  for name in "${selected[@]}"; do
    if [[ "${MODULE_RESTART_GATEWAY[$name]:-}" == "true" ]]; then
      local st
      st="$(state_get_module "$name")"
      [[ "$st" == "installed" ]] && need_restart=1
    fi
  done

  if [[ "$need_restart" -eq 1 ]]; then
    echo ""
    if prompt_yesno "部分模块安装/更新后需要重启 OpenClaw gateway，是否立即重启?" "Y/N"; then
      log_info "正在重启 OpenClaw gateway..."
      openclaw gateway restart || log_warn "OpenClaw gateway 重启失败，请手动执行"
    else
      log_warn "已跳过重启，请稍后手动执行: openclaw gateway restart"
    fi
  fi

  return $failed
}

# ============================================================
# [Section 8] 非交互模式（参数解析）
# ============================================================

usage() {
  cat <<EOF
Usage: $SCRIPT_NAME [OPTIONS]

安装模式:
  (无参数)                 TTY 下进入交互模式；非 TTY 下默认 recommended
  -i, --interactive        交互模式（引导选择安装模式）
  --mode recommended       推荐安装（安全 + 提速 + 省钱）
  --mode all               完整安装（所有模块）
  --mode MODULE [MODULE..] 自定义安装指定模块

可选模块:
  openclaw      OpenClaw 本体（Agent Gateway）
  osbase        系统内存自动优化（小规格实例自动调优）
  skill         预置技能库（网络诊断/日志分析/部署辅助等）
  skillfs       精简 Skill 文件系统（降低 Token 开销）
  ws-ckpt       会话快照与恢复
  sec-core      安全运行时防护（sandbox + asset-verify + skill）
  sight         可观测性（eBPF Agent 行为监控 + Token 统计）
  token-less    Token 节约（上下文压缩 + 输出过滤 + RTK 命令重写）
  skvm          skvm skill bank

安装模式包含模块:
  recommended  osbase, skill, skillfs, ws-ckpt, skvm, sec-core, token-less
  all          osbase, skill, skillfs, ws-ckpt, sec-core, sight, token-less, skvm
  MODULE...    指定一个或多个模块，例如: --mode sec-core token-less skillfs

说明:
  openclaw 是基础依赖；当所选模块依赖 OpenClaw 且本机未安装时，脚本会自动加入安装队列。

示例:
  $SCRIPT_NAME                            TTY 交互 / 非 TTY 推荐安装
  $SCRIPT_NAME --mode recommended         非交互推荐安装
  $SCRIPT_NAME --mode all                 非交互完整安装
  $SCRIPT_NAME --mode osbase skill        非交互安装指定模块
  $SCRIPT_NAME --mode recommended --dry-run  预览推荐安装计划

操作:
  --list                   列出可用模块和能力
  --status                 查看当前安装状态
  --rollback               回滚失败的模块
  --rollback MODULE [...]  回滚指定模块
  --rollback --full        全量回滚（恢复备份配置）
  --retry                  重试失败的模块

选项:
  --dry-run                仅预览，不执行
  --install-deps           自动安装缺失的 RPM 依赖（dnf install -y）
  --base-dir DIR           Anolisa 组件根路径 (默认: $AGENTICOS_BASE)
  --config FILE            OpenClaw 配置文件路径 (默认: $OPENCLAW_CONFIG)
  -v, --verbose            详细输出
  -h, --help               帮助信息
EOF
}

SELECTED_MODULES=()
SELECTED_SKILL_PATHS=()
PRESELECTED_SKILL_PATHS=()
SKILL_SELECT_MODE=0
ACTION="install"
ROLLBACK_MODULES=()
FULL_ROLLBACK=0
DRY_RUN=0
VERBOSE=0
AUTO_INSTALL_RPM=0

parse_args() {
  while [[ $# -gt 0 ]]; do
    case "$1" in
      -i|--interactive) ACTION="interactive" ;;
      --mode)
        if [[ $# -lt 2 ]]; then
          die "--mode 需要指定模式名称或模块列表 (可选模式: recommended, all; 或模块名)"
        fi
        shift
        local mode_name="$1"
        # 检查是否为注册的预设模式
        if [[ -n "${MODE_MODULES[$mode_name]:-}" ]]; then
          if [[ "$mode_name" == "custom" ]]; then
            die "--mode 不支持 custom 模式，请使用 -i 进入交互模式"
          fi
          resolve_mode_modules "$mode_name"
        else
          # 非预设模式名，当作模块名列表处理
          # 验证第一个参数是有效模块名
          local valid=0
          for m in "${MODULES[@]}"; do
            [[ "$m" == "$mode_name" ]] && { valid=1; break; }
          done
          if [[ "$valid" -eq 0 ]]; then
            die "未知的模式或模块: $mode_name (可选模式: recommended, all; 可选模块: ${MODULES[*]})"
          fi
          SELECTED_MODULES+=("$mode_name")
          # 继续消费后续的模块名参数
          while [[ $# -gt 1 ]]; do
            local next="${2:-}"
            # 如果下一个参数以 - 开头，说明是其他选项，停止消费
            [[ "$next" == -* ]] && break
            valid=0
            for m in "${MODULES[@]}"; do
              [[ "$m" == "$next" ]] && { valid=1; break; }
            done
            [[ "$valid" -eq 0 ]] && break
            shift
            SELECTED_MODULES+=("$1")
          done
        fi
        ACTION="install"
        ;;
      --list)       ACTION="list" ;;
      --status)     ACTION="status" ;;
      --rollback)
        ACTION="rollback"
        # 贪婪消费后续非 -- 开头的参数作为模块名
        while [[ $# -gt 1 ]]; do
          local next="${2:-}"
          [[ "$next" == -* ]] && break
          shift
          ROLLBACK_MODULES+=("$1")
        done
        ;;
      --full)       FULL_ROLLBACK=1 ;;
      --retry)      ACTION="retry" ;;
      --dry-run)    DRY_RUN=1 ;;
      --install-deps) AUTO_INSTALL_RPM=1 ;;
      --base-dir)
        [[ $# -ge 2 ]] || die "--base-dir 需要指定目录"
        shift
        AGENTICOS_BASE="$1"
        ;;
      --config)
        [[ $# -ge 2 ]] || die "--config 需要指定配置文件路径"
        shift
        OPENCLAW_CONFIG="$1"
        ;;
      -v|--verbose) VERBOSE=1 ;;
      -h|--help)    usage; exit 0 ;;
      *)            log_error "Unknown option: $1"; usage; exit 1 ;;
    esac
    shift
  done
}

# ============================================================
# [Section 9] 操作命令
# ============================================================

action_list() {
  echo -e "${BOLD}AgenticOS for OpenClaw — 可用模块${NC}"
  echo ""
  if command -v column &>/dev/null; then
    {
      printf "模块\t分类\t安装方式\t描述\n"
      for name in "${MODULES[@]}"; do
        printf "%s\t%s\t%s\t%s\n" \
          "$name" "${MODULE_CATEGORY[$name]}" "$(install_label "$name")" "${MODULE_DESC[$name]}"
      done
    } | column -t -s $'\t'
  else
    printf "  %-12s %-8s %-24s %s\n" "模块" "分类" "安装方式" "描述"
    printf "  %-12s %-8s %-24s %s\n" "────" "────" "────────────────────────" "────"
    for name in "${MODULES[@]}"; do
      printf "  %-12s %-8s %-24s %s\n" \
        "$name" "${MODULE_CATEGORY[$name]}" "$(install_label "$name")" "${MODULE_DESC[$name]}"
    done
  fi
  echo ""
  echo "  组件根路径: $AGENTICOS_BASE"
  echo "  配置文件:   $OPENCLAW_CONFIG"
  echo "  状态文件:   $STATE_FILE"
}

action_status() {
  echo -e "${BOLD}AgenticOS for OpenClaw — 安装状态${NC}"
  echo ""

  if [[ ! -f "$STATE_FILE" ]]; then
    log_warn "尚未执行过安装，以下状态来自实时检测"
  fi

  if command -v column &>/dev/null; then
    {
      printf "模块\t状态\t时间\t错误\n"
      for name in "${MODULES[@]}"; do
        local st ts err
        local st_display
        local detect_fn="detect_${name//-/_}"
        st="$(state_get_module "$name")"
        ts="$(state_get_module "$name" "ts")"
        err="$(state_get_module "$name" "error")"

        if [[ -z "$st" ]]; then
          if declare -F "$detect_fn" >/dev/null && "$detect_fn" &>/dev/null; then
            st_display="已安装(检测)"
          else
            st_display="未安装"
          fi
        else
          case "$st" in
            installed)      st_display="已安装" ;;
            failed)         st_display="安装失败" ;;
            cleaned)        st_display="已回滚" ;;
            cleanup-failed) st_display="回滚失败" ;;
            *)              st_display="$st" ;;
          esac
        fi

        printf "%s\t%s\t%s\t%s\n" "$name" "$st_display" "${ts:---}" "${err:---}"
      done
    } | column -t -s $'\t'
  else
    printf "  %-12s %-12s %-20s %s\n" "模块" "状态" "时间" "错误"
    printf "  %-12s %-12s %-20s %s\n" "────" "────" "────────────────────" "────"
    for name in "${MODULES[@]}"; do
      local st ts err
      local st_display
      local detect_fn="detect_${name//-/_}"
      st="$(state_get_module "$name")"
      ts="$(state_get_module "$name" "ts")"
      err="$(state_get_module "$name" "error")"

      if [[ -z "$st" ]]; then
        if declare -F "$detect_fn" >/dev/null && "$detect_fn" &>/dev/null; then
          st_display="已安装(检测)"
        else
          st_display="未安装"
        fi
      else
        case "$st" in
          installed)      st_display="已安装" ;;
          failed)         st_display="安装失败" ;;
          cleaned)        st_display="已回滚" ;;
          cleanup-failed) st_display="回滚失败" ;;
          *)              st_display="$st" ;;
        esac
      fi

      printf "  %-12s %-12s %-20s %s\n" "$name" "$st_display" "${ts:---}" "${err:---}"
    done
  fi
}

action_rollback() {
  state_init
  local rollback_failures=0
  local -a rolled_modules=()

  if [[ ${#ROLLBACK_MODULES[@]} -gt 0 ]]; then
    # 指定模块回滚
    log_step "回滚指定模块: ${ROLLBACK_MODULES[*]}"
    for name in "${ROLLBACK_MODULES[@]}"; do
      local status
      status="$(state_get_module "$name")"
      if [[ -z "$status" ]]; then
        log_warn "$name — 未在安装记录中找到，跳过"
        continue
      fi
      rolled_modules+=("$name")
      run_module_cleanup "$name" || rollback_failures=$((rollback_failures + 1))
    done
  elif [[ "$FULL_ROLLBACK" -eq 1 ]]; then
    log_step "全量回滚"
    for name in $(state_list_modules); do
      rolled_modules+=("$name")
      run_module_cleanup "$name" || rollback_failures=$((rollback_failures + 1))
    done
    # 插件卸载依赖配置文件
    CURRENT_MODULE="full-rollback"
    restore_backup
    finalize_full_rollback
    CURRENT_MODULE=""
    log_info "全量回滚完成"
  else
    log_step "回滚失败模块"
    local found=0
    for name in $(state_list_modules); do
      if [[ "$(state_get_module "$name")" == "failed" ]]; then
        rolled_modules+=("$name")
        run_module_cleanup "$name" || rollback_failures=$((rollback_failures + 1))
        found=$((found + 1))
      fi
    done
    [[ "$found" -eq 0 ]] && log_info "没有需要回滚的失败模块"
  fi

  log_step "回滚报告"
  if [[ ${#rolled_modules[@]} -eq 0 ]]; then
    echo "  无回滚组件"
    print_divider
  else
    for name in "${rolled_modules[@]}"; do
      local st
      st="$(state_get_module "$name")"
      echo "  - 组件: $name  状态: ${st:-unknown}"
      if [[ -n "${MODULE_EXEC_CMDS[$name]:-}" ]]; then
        echo "    执行命令:"
        while IFS= read -r cmd_line; do
          [[ -n "$cmd_line" ]] && echo "      * $cmd_line"
        done <<< "${MODULE_EXEC_CMDS[$name]}"
      else
        echo "    执行命令: (无外部 shell 命令或未触发)"
      fi
      print_divider
    done
  fi

  if [[ -n "${MODULE_EXEC_CMDS[full-rollback]:-}" ]]; then
    echo "  - 组件: full-rollback  状态: completed"
    echo "    执行命令:"
    while IFS= read -r cmd_line; do
      [[ -n "$cmd_line" ]] && echo "      * $cmd_line"
    done <<< "${MODULE_EXEC_CMDS[full-rollback]}"
    print_divider
  fi

  if [[ "$rollback_failures" -gt 0 ]]; then
    log_warn "回滚过程中有 $rollback_failures 个模块清理失败，请检查日志"
    return 1
  fi
}

action_retry() {
  state_init
  log_step "重试失败模块"

  local -a retry_modules=()
  for name in $(state_list_modules); do
    if [[ "$(state_get_module "$name")" == "failed" ]]; then
      retry_modules+=("$name")
    fi
  done

  if [[ ${#retry_modules[@]} -eq 0 ]]; then
    log_info "没有需要重试的失败模块"
    return 0
  fi

  run_install_batch "${retry_modules[@]}"
}

action_install() {
  if [[ ${#SELECTED_MODULES[@]} -eq 0 ]]; then
    log_error "未指定任何模块，使用 --mode recommended 或 --mode MODULE..."
    usage
    exit 1
  fi

  prerequisites_check || die "前置检测未通过"
  json_init "$OPENCLAW_CONFIG"
  state_init
  backup_file "$OPENCLAW_CONFIG" >/dev/null

  log_step "执行安装 (非交互模式)"
  local failed=0
  run_install_batch "${SELECTED_MODULES[@]}" || failed=$?
  normalize_openclaw_plugin_config

  log_step "安装报告"
  local ok=0 skip=0 fail=0
  for name in "${SELECTED_MODULES[@]}"; do
    local st
    st="$(state_get_module "$name")"
    local status_text
    case "$st" in
      installed) status_text="installed"; ok=$((ok + 1)) ;;
      failed)    status_text="failed"; fail=$((fail + 1)) ;;
      *)         status_text="skipped"; skip=$((skip + 1)) ;;
    esac

    echo "  - 组件: $name  状态: $status_text"
    if [[ -n "${MODULE_EXEC_CMDS[$name]:-}" ]]; then
      echo "    执行命令:"
      while IFS= read -r cmd_line; do
        [[ -n "$cmd_line" ]] && echo "      * $cmd_line"
      done <<< "${MODULE_EXEC_CMDS[$name]}"
    else
      echo "    执行命令: (无外部 shell 命令或未触发)"
    fi
    print_divider
  done

  if [[ "$failed" -eq 0 ]]; then
    log_info "所有模块安装成功"
  else
    log_warn "$failed 个模块安装失败，使用 --status 查看详情"
  fi

  # 检查是否有模块需要重启 OpenClaw gateway
  local need_restart=0
  for name in "${SELECTED_MODULES[@]}"; do
    if [[ "${MODULE_RESTART_GATEWAY[$name]:-}" == "true" ]]; then
      local st
      st="$(state_get_module "$name")"
      [[ "$st" == "installed" ]] && need_restart=1
    fi
  done

  if [[ "$need_restart" -eq 1 ]]; then
    log_info "正在重启 OpenClaw gateway..."
    openclaw gateway restart || log_warn "OpenClaw gateway 重启失败，请手动执行: openclaw gateway restart"
  fi

  return $failed
}

# ============================================================
# [Section 10] 主入口
# ============================================================

main() {
  if [[ $# -eq 0 ]]; then
    if [[ -t 0 ]]; then
      ACTION="interactive"
    else
      resolve_mode_modules "recommended"
      ACTION="install"
    fi
  else
    parse_args "$@"
  fi

  refresh_module_paths

  case "$ACTION" in
    interactive) interactive_mode ;;
    list)        action_list ;;
    status)      action_status ;;
    rollback)    action_rollback ;;
    retry)       action_retry ;;
    install)     action_install ;;
  esac
}

main "$@"
