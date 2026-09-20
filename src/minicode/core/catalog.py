"""Model catalog: the models minicode knows how to drive.

Each entry records the provider that serves the model, its context window
and whether it accepts a reasoning-effort parameter. The catalog is the
single source of truth for ``/model`` completion, context-window load
reporting and provider construction; unknown models fall back to a
conservative default window so the status bar never crashes on them.
"""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class ModelInfo:
    """One drivable model and the facts the harness needs about it."""

    name: str  # provider-facing model id, e.g. "z.ai/glm-5.3-flash"
    provider: str  # provider key: commandcode / anthropic / fake
    context_window: int  # prompt tokens the model accepts
    supports_effort: bool = False  # accepts a reasoning_effort parameter


#: Z.ai gateway models share the CommandCode (OpenAI-compatible) endpoint;
#: DeepSeek V4.1 Flash and GLM-5.3 Flash both expose a 1M-token window.
_GLM_5_3_FLASH = ModelInfo(
    name="z.ai/glm-5.3-flash",
    provider="commandcode",
    context_window=1_000_000,
    supports_effort=True,
)
_DEEPSEEK_V41_FLASH = ModelInfo(
    name="deepseek/deepseek-v4.1-flash",
    provider="commandcode",
    context_window=1_000_000,
    supports_effort=True,
)
_CLAUDE_SONNET_4_5 = ModelInfo(
    name="claude-sonnet-4-5",
    provider="anthropic",
    context_window=200_000,
)

#: Registry keyed by model id; ``default_model`` names the CommandCode default.
MODEL_CATALOG: dict[str, ModelInfo] = {
    _DEEPSEEK_V41_FLASH.name: _DEEPSEEK_V41_FLASH,
    _GLM_5_3_FLASH.name: _GLM_5_3_FLASH,
    _CLAUDE_SONNET_4_5.name: _CLAUDE_SONNET_4_5,
}

DEFAULT_MODEL = _DEEPSEEK_V41_FLASH.name

#: Fallback for models not in the catalog (user-supplied --model values).
_UNKNOWN_MODEL_WINDOW = 200_000

#: Reasoning-effort levels accepted by effort-capable models, cheapest first.
EFFORT_LEVELS: tuple[str, ...] = ("off", "low", "medium", "high", "xhigh", "max")


def parse_effort(text: str) -> str | None:
    """Normalize *text* to a valid effort level, or ``None`` when invalid."""
    normalized = text.strip().lower()
    return normalized if normalized in EFFORT_LEVELS else None


def lookup_model(name: str) -> ModelInfo:
    """Return the catalog entry for *name*, or an anthropic-style fallback.

    Unknown model ids are still drivable (gateways route by name); they only
    get the conservative fallback context window.
    """
    known = MODEL_CATALOG.get(name)
    if known is not None:
        return known
    provider = "anthropic" if name.startswith("claude") else "commandcode"
    return ModelInfo(name=name, provider=provider, context_window=_UNKNOWN_MODEL_WINDOW)


def models_for_provider(provider: str) -> list[ModelInfo]:
    """Catalog models served by *provider*, in catalog order."""
    return [info for info in MODEL_CATALOG.values() if info.provider == provider]
