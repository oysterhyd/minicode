"""One routing implementation for configured models in every local host."""

from __future__ import annotations

from minicode.configuration import HarnessConfiguration
from minicode.providers.errors import ProviderRequestError


def model_catalog():
    from minicode.core.catalog import MODEL_CATALOG, ModelInfo
    catalog = dict(MODEL_CATALOG)
    configuration = HarnessConfiguration()
    for service in configuration.read()["services"]:
        for model in service["models"]:
            key = configuration.model_key(service, model)
            catalog[key] = ModelInfo(
                name=key, provider="anthropic" if service["apiStyle"] == "anthropic" else "commandcode",
                context_window=model["contextWindow"], max_output_tokens=model["maxOutputTokens"],
                supports_effort=model["supportsEffort"],
            )
    return catalog


def configured_provider(model: str, *, configuration: HarnessConfiguration | None = None,
                        effort: str | None = None, fallback=None):
    configuration = configuration or HarnessConfiguration()
    configured = configuration.find_model(model)
    if configured is None:
        if fallback is not None:
            return fallback(model, effort or "off")
        raise ProviderRequestError(f"model is not configured: {model}")
    service, info = configured
    url, key = configuration.credentials(service)
    if not service["enabled"]:
        raise ProviderRequestError("请启用服务并配置 API 密钥")
    if fallback is not None and service.get("builtin") and not service.get("apiKey"):
        provider = fallback(model, effort or "off")
        if hasattr(provider, "base_url"):
            provider.base_url = url
        provider.max_tokens = info["maxOutputTokens"]
        if hasattr(provider, "reasoning_effort") and not info["supportsEffort"]:
            provider.reasoning_effort = None
    elif not key:
        raise ProviderRequestError("请启用服务并配置 API 密钥")
    elif service["apiStyle"] == "anthropic":
        from minicode.providers.anthropic_provider import AnthropicProvider
        provider = AnthropicProvider(model=info["modelId"], api_key=key, base_url=url,
                                     max_tokens=info["maxOutputTokens"])
    else:
        from minicode.providers.commandcode import CommandCodeProvider
        provider = CommandCodeProvider(model=info["modelId"], api_key=key, base_url=url,
                                       max_tokens=info["maxOutputTokens"],
                                       reasoning_effort=effort if info["supportsEffort"] and effort != "off" else None)
    provider.configuration_id = model
    provider.context_window = info["contextWindow"]
    return provider
