import asyncio

from minicode.core.models import Message, ModelResponse, TextBlock
from minicode.providers.base import ReasoningDelta, ResponseDone
from minicode.providers.errors import ProviderError
from minicode.runtime.streaming import stream_response


def test_partial_reasoning_failure_does_not_retry_or_merge_into_answer():
    class Provider:
        calls = 0
        async def stream(self, **kwargs):
            self.calls += 1
            yield ReasoningDelta("partial thought")
            raise ProviderError("connection lost")
    provider = Provider()
    reasoning, retries = [], []
    async def observe(text): reasoning.append(text)
    async def retry(*args): retries.append(args)
    result = asyncio.run(stream_response(provider, system="", messages=[], tools=[], on_delta=None,
                                        on_reasoning=observe, on_retry=retry, settled=lambda _: False))
    assert result.error and provider.calls == 1 and not retries
    assert reasoning == ["partial thought"] and result.response is None


def test_reasoning_capture_is_bounded_and_observer_failure_is_isolated(monkeypatch):
    from minicode.runtime import streaming
    class Provider:
        async def stream(self, **kwargs):
            yield ReasoningDelta("thought")
            yield ResponseDone(ModelResponse(blocks=[TextBlock(text="answer")]))
    async def failing_observer(text): raise RuntimeError("view closed")
    async def retry(*args): pass
    args = dict(system="", messages=[Message(role="user", content=[TextBlock(text="hello")])], tools=[],
                on_delta=None, on_reasoning=failing_observer, on_retry=retry, settled=lambda _: False)
    result = asyncio.run(stream_response(Provider(), **args))
    assert result.reasoning == "thought" and result.response.text == "answer"
    monkeypatch.setattr(streaming, "MAX_RESPONSE_BYTES", 3)
    result = asyncio.run(stream_response(Provider(), **args))
    assert result.response is None and "capture limit" in str(result.error)
