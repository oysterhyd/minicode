"""Incremental statistics over the append-only trace, with bounded samples."""
from collections import deque

from minicode.core.models import EventType


class SessionStatistics:
    def __init__(self):
        self.session_id = None
        self.after_seq = -1
        self.samples = deque(maxlen=32)
        self.tool_calls = self.requests = 0
        self.seconds = self.generation_seconds = 0.0
        self.measured_output = self.generated_output = 0

    def read(self, store, session_id):
        if session_id != self.session_id:
            self.__init__()
            self.session_id = session_id
        latest = store.latest_event_seq(session_id) if session_id else -1
        cursor = self.after_seq
        while cursor < latest:
            events = store.get_events(session_id, after_seq=cursor, limit=512,
                                      types=(EventType.ASSISTANT_MESSAGE, EventType.TOOL_CALL_START))
            if not events:
                break
            for event in events:
                if event.type == EventType.TOOL_CALL_START:
                    self.tool_calls += 1
                    continue
                data = event.data
                usage = data.get("usage", {})
                seconds, generation = data.get("request_seconds"), data.get("generation_seconds")
                available = usage.get("available", True)
                output = usage.get("output_tokens", 0)
                self.requests += 1
                request_tps = output / seconds if seconds and seconds > 0 and available else None
                tps = output / generation if generation and generation > 0 and available else None
                self.samples.append({"round": self.requests, "input": usage.get("input_tokens", 0),
                    "output": output, "cached": usage.get("cache_read_tokens", 0), "available": available,
                    "seconds": seconds, "generationSeconds": generation,
                    "firstTokenSeconds": data.get("first_token_seconds"), "requestTps": request_tps, "tps": tps})
                if request_tps is not None:
                    self.seconds += seconds
                    self.measured_output += output
                if tps is not None:
                    self.generation_seconds += generation
                    self.generated_output += output
            cursor = events[-1].seq
        self.after_seq = latest
        return {"toolCalls": self.tool_calls, "requests": self.requests, "modelSeconds": self.seconds,
                "tps": self.generated_output / self.generation_seconds if self.generation_seconds else None,
                "requestTps": self.measured_output / self.seconds if self.seconds else None,
                "lastTps": self.samples[-1]["tps"] if self.samples else None,
                "samples": [dict(sample) for sample in self.samples]}
