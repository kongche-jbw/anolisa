#!/usr/bin/env python3
"""
OpenClaw 4.26 configuration helper.

Writes ~/.openclaw/openclaw.json for:
- Qwen built-in provider compatible defaults
- Standard or Coding Plan endpoint alignment
- DingTalk connector channel
"""

import argparse
import json
import os
import sys
from pathlib import Path


DEFAULT_CONFIG_PATH = Path("~/.openclaw/openclaw.json").expanduser()

ENDPOINTS = {
    ("standard", "china"): {
        "auth_choice": "qwen-standard-api-key-cn",
        "base_url": "https://dashscope.aliyuncs.com/compatible-mode/v1",
    },
    ("standard", "global"): {
        "auth_choice": "qwen-standard-api-key",
        "base_url": "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
    },
    ("coding", "china"): {
        "auth_choice": "qwen-api-key-cn",
        "base_url": "https://coding.dashscope.aliyuncs.com/v1",
    },
    ("coding", "global"): {
        "auth_choice": "qwen-api-key",
        "base_url": "https://coding-intl.dashscope.aliyuncs.com/v1",
    },
}


def deep_merge(base, override):
    result = dict(base)
    for key, value in override.items():
        if key in result and isinstance(result[key], dict) and isinstance(value, dict):
            result[key] = deep_merge(result[key], value)
        else:
            result[key] = value
    return result


def merge_plugin_allow(existing, merged):
    existing_allow = existing.get("plugins", {}).get("allow", [])
    merged_allow = merged.get("plugins", {}).get("allow")
    if merged_allow is None:
        return merged

    combined = []
    for plugin_id in [*existing_allow, *merged_allow]:
        if plugin_id and plugin_id not in combined:
            combined.append(plugin_id)

    merged.setdefault("plugins", {})["allow"] = combined
    return merged


def qwen_model_ref(model_id):
    return model_id if model_id.startswith("qwen/") else f"qwen/{model_id}"


def strip_qwen_prefix(model_id):
    if model_id.startswith("qwen/"):
        return model_id[len("qwen/") :]
    return model_id


def build_qwen_provider(args, api_key):
    endpoint = ENDPOINTS[(args.plan, args.region)]
    model_id = strip_qwen_prefix(args.model_id)

    provider = {
        "baseUrl": endpoint["base_url"],
        "apiKey": api_key,
        "api": "openai-completions",
        "models": [
            {
                "id": model_id,
                "name": model_id,
                "reasoning": args.reasoning,
                "input": ["text", "image"]
                if model_id in {"qwen3.5-plus", "qwen3.6-plus", "qwen3-coder-plus"}
                else ["text"],
                "contextWindow": args.context_window,
                "maxTokens": args.max_tokens,
                "cost": {
                    "input": 0,
                    "output": 0,
                    "cacheRead": 0,
                    "cacheWrite": 0,
                },
            }
        ],
    }

    if not args.write_provider_config:
        return None
    return provider


def build_dingtalk_channel(args):
    channel = {
        "enabled": True,
        "clientId": args.dingtalk_client_id,
        "clientSecret": args.dingtalk_client_secret,
        "sharedMemoryAcrossConversations": args.shared_memory_across_conversations,
        "separateSessionByConversation": args.separate_session_by_conversation,
        "groupSessionScope": args.group_session_scope,
    }

    if args.dingtalk_robot_code:
        channel["robotCode"] = args.dingtalk_robot_code
    if args.dingtalk_corp_id:
        channel["corpId"] = args.dingtalk_corp_id
    if args.dingtalk_agent_id:
        channel["agentId"] = args.dingtalk_agent_id

    return channel


def build_config(args):
    api_key = (
        args.qwen_api_key
        or args.modelstudio_api_key
        or args.dashscope_api_key
        or os.environ.get("QWEN_API_KEY")
        or os.environ.get("MODELSTUDIO_API_KEY")
        or os.environ.get("DASHSCOPE_API_KEY")
        or ""
    )
    if not api_key:
        raise SystemExit(
            "Qwen API key is required. Pass --qwen-api-key or set QWEN_API_KEY."
        )

    model_ref = qwen_model_ref(args.model_id)
    provider_config = build_qwen_provider(args, api_key)

    config = {
        "agents": {
            "defaults": {
                "model": {
                    "primary": model_ref,
                },
                "models": {
                    model_ref: {
                        "alias": args.model_alias,
                    }
                },
                "maxConcurrent": args.max_concurrent,
                "subagents": {
                    "maxConcurrent": args.subagent_max_concurrent,
                },
            }
        },
        "commands": {
            "native": "auto",
            "nativeSkills": "auto",
            "restart": True,
            "ownerDisplay": "raw",
        },
        "session": {
            "dmScope": "per-channel-peer",
        },
        "gateway": {
            "mode": "local",
        },
        "plugins": {
            "enabled": True,
            "allow": ["qwen"],
        },
        "skills": {
            "load": {
                "extraDirs": [
                    "/usr/share/anolisa/skills",
                ]
            }
        },
    }

    if provider_config is not None:
        config["models"] = {
            "mode": "merge",
            "providers": {
                "qwen": provider_config,
            },
        }

    if args.dingtalk_client_id or args.dingtalk_client_secret:
        if not args.dingtalk_client_id or not args.dingtalk_client_secret:
            raise SystemExit(
                "Both --dingtalk-client-id and --dingtalk-client-secret are required when configuring DingTalk."
            )
        config["plugins"] = {
            "enabled": True,
            "allow": ["qwen", "dingtalk-connector"],
            "entries": {
                "dingtalk-connector": {
                    "enabled": True,
                }
            },
        }
        config["channels"] = {
            "dingtalk-connector": build_dingtalk_channel(args),
        }

    return config


def apply_config(config, config_path):
    print("\n--- Writing OpenClaw 4.26 config ---\n")

    existing = {}
    if config_path.exists():
        with config_path.open("r", encoding="utf-8") as fh:
            existing = json.load(fh)

    merged = deep_merge(existing, config)
    merged = merge_plugin_allow(existing, merged)

    config_path.parent.mkdir(parents=True, exist_ok=True)
    with config_path.open("w", encoding="utf-8") as fh:
        json.dump(merged, fh, indent=2, ensure_ascii=False)
        fh.write("\n")

    for key in config:
        print(f"  [OK] {key}")

    print(f"\nConfig written: {config_path}")


def parse_args():
    parser = argparse.ArgumentParser(
        description="Configure OpenClaw 4.26 with Qwen and DingTalk Connector."
    )

    parser.add_argument(
        "--config",
        default=str(DEFAULT_CONFIG_PATH),
        help="OpenClaw config path",
    )

    parser.add_argument(
        "--plan",
        default="standard",
        choices=["standard", "coding"],
        help="Qwen plan type: standard pay-as-you-go or coding subscription",
    )
    parser.add_argument(
        "--region",
        default="china",
        choices=["china", "global"],
        help="Qwen endpoint region",
    )
    parser.add_argument(
        "--model-id",
        default="qwen3.5-plus",
        help="Qwen model id, without or with qwen/ prefix",
    )
    parser.add_argument(
        "--model-alias",
        default="qwen-default",
        help="Alias written under agents.defaults.models",
    )
    parser.add_argument(
        "--qwen-api-key",
        default="",
        help="Preferred Qwen API key. Also accepted through QWEN_API_KEY.",
    )
    parser.add_argument(
        "--modelstudio-api-key",
        default="",
        help="Compatibility API key alias.",
    )
    parser.add_argument(
        "--dashscope-api-key",
        default="",
        help="Compatibility API key alias.",
    )
    parser.add_argument(
        "--write-provider-config",
        dest="write_provider_config",
        action="store_true",
        default=True,
        help=(
            "Write models.providers.qwen with selected endpoint and key. "
            "Keep enabled for non-interactive agent installation. "
            "Use --no-write-provider-config only for manual onboard/auth-store flows."
        ),
    )
    parser.add_argument(
        "--no-write-provider-config",
        dest="write_provider_config",
        action="store_false",
    )
    parser.add_argument("--context-window", type=int, default=1_000_000)
    parser.add_argument("--max-tokens", type=int, default=65_536)
    parser.add_argument("--reasoning", action="store_true")

    parser.add_argument("--dingtalk-client-id", default="")
    parser.add_argument("--dingtalk-client-secret", default="")
    parser.add_argument("--dingtalk-robot-code", default="")
    parser.add_argument("--dingtalk-corp-id", default="")
    parser.add_argument("--dingtalk-agent-id", default="")
    parser.add_argument(
        "--shared-memory-across-conversations",
        dest="shared_memory_across_conversations",
        action="store_true",
        default=True,
    )
    parser.add_argument(
        "--no-shared-memory-across-conversations",
        dest="shared_memory_across_conversations",
        action="store_false",
    )
    parser.add_argument(
        "--separate-session-by-conversation",
        dest="separate_session_by_conversation",
        action="store_true",
        default=True,
    )
    parser.add_argument(
        "--no-separate-session-by-conversation",
        dest="separate_session_by_conversation",
        action="store_false",
    )
    parser.add_argument(
        "--group-session-scope",
        default="group",
        choices=["group", "group_sender"],
    )
    parser.add_argument("--max-concurrent", type=int, default=4)
    parser.add_argument("--subagent-max-concurrent", type=int, default=8)

    return parser.parse_args()


def main():
    args = parse_args()

    if args.plan == "coding" and strip_qwen_prefix(args.model_id) == "qwen3.6-plus":
        print(
            "Warning: qwen3.6-plus should prefer the standard endpoint; "
            "Coding Plan availability may lag behind the public catalog.",
            file=sys.stderr,
        )

    config = build_config(args)
    apply_config(config, Path(args.config).expanduser())

    endpoint = ENDPOINTS[(args.plan, args.region)]
    print("\nNext steps:")
    if args.dingtalk_client_id and args.dingtalk_client_secret:
        print("  openclaw plugins install @dingtalk-real-ai/dingtalk-connector")
    print("  openclaw gateway --force")
    print("  openclaw models list --provider qwen")
    print(
        f"  # Manual alternative only: openclaw onboard --auth-choice {endpoint['auth_choice']}"
    )
    if args.dingtalk_client_id and args.dingtalk_client_secret:
        print("  openclaw channels status --probe")


if __name__ == "__main__":
    main()
