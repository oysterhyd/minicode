"""TUI admission, approval and shutdown regressions (headless Textual Pilot)."""
from __future__ import annotations

import asyncio

from minicode.core.models import ModelResponse, TextBlock, Usage
from minicode.providers import ResponseDone, TextDelta
from minicode.ui.app import ApprovalModal, PromptArea
from test_tui import _make_app, _create_file_script, _wait_for_modal, _wait_turn_done


class PausingProvider:
    name = "fake"
    model = "fake"
    timeout_s = 10

    def __init__(self):
        self.started = asyncio.Event()
        self.release = asyncio.Event()
        self.closed = False
        self.requests = 0

    async def stream(self, **_):
        self.requests += 1
        self.started.set()
        yield TextDelta("partial")
        try:
            await self.release.wait()
            yield ResponseDone(ModelResponse(blocks=[TextBlock(text="done")], usage=Usage()))
        finally:
            await asyncio.sleep(.05)

    async def aclose(self):
        self.closed = True


def test_burst_submissions_reserve_one_worker_and_queue_second_input(tmp_path):
    async def scenario():
        app, _ = _make_app(tmp_path, [])
        provider = PausingProvider()
        app._runtime.set_model(provider=provider, provider_name="fake", model="fake")
        try:
            async with app.run_test() as pilot:
                prompt = app.query_one("#prompt", PromptArea)
                app.post_message(PromptArea.Submitted("one", prompt))
                app.post_message(PromptArea.Submitted("two", prompt))
                await asyncio.wait_for(provider.started.wait(), 3)
                await pilot.pause(.05)
                assert app._busy
                assert list(app._queued_turns) == ["two"]
                assert provider.requests == 1
                provider.release.set()
                await _wait_turn_done(app)
                await pilot.pause()
                await _wait_turn_done(app)
                assert provider.requests == 2
                users = [b.text for m in app._runtime.messages if m.role == "user" for b in m.content if isinstance(b, TextBlock)]
                assert users == ["one", "two"]
                assert not any("执行失败" in t for t in app._log_texts())
        finally:
            app._store.close()
    asyncio.run(scenario())


def test_quit_awaits_delayed_cancellation_and_does_not_drain_queue(tmp_path):
    async def scenario():
        app, _ = _make_app(tmp_path, [])
        provider = PausingProvider()
        app._runtime.set_model(provider=provider, provider_name="fake", model="fake")
        try:
            async with app.run_test() as pilot:
                prompt = app.query_one("#prompt", PromptArea)
                app.post_message(PromptArea.Submitted("one", prompt))
                await asyncio.wait_for(provider.started.wait(), 3)
                app.post_message(PromptArea.Submitted("two", prompt))
                await pilot.pause(.02)
                await pilot.press("ctrl+q")
            assert provider.closed
            assert not app._runtime.snapshot().running
            assert provider.requests == 1
            assert app._store.get_session(app._runtime.session_id).exit_reason == "cancelled"
        finally:
            app._store.close()
    asyncio.run(scenario())


def test_retired_cleanup_failure_is_observed_before_task_is_discarded(tmp_path):
    import gc
    async def scenario():
        app, _ = _make_app(tmp_path, [])
        unhandled = []
        loop = asyncio.get_running_loop()
        loop.set_exception_handler(lambda _loop, context: unhandled.append(context))
        async def failed_close():
            raise RuntimeError("injected retired provider close failure")
        try:
            app._track_cleanup(failed_close())
            for _ in range(4):
                await asyncio.sleep(0)
            gc.collect()
            assert not app._cleanup_tasks
            assert not unhandled
        finally:
            app._store.close()
    asyncio.run(scenario())


def test_approval_deadline_removes_modal_and_keeps_target_unchanged(tmp_path):
    async def scenario():
        app, workspace = _make_app(tmp_path, _create_file_script())
        app._runtime.set_budget(app._runtime.budget.model_copy(update={"max_seconds": .5}))
        try:
            async with app.run_test() as pilot:
                prompt = app.query_one("#prompt", PromptArea)
                app.post_message(PromptArea.Submitted("write", prompt))
                await _wait_for_modal(app)
                await _wait_turn_done(app)
                await pilot.pause()
                assert not any(isinstance(s, ApprovalModal) for s in app.screen_stack)
                assert not (workspace / "new.txt").exists()
                assert app._store.get_session(app._runtime.session_id).exit_reason == "time_budget"
        finally:
            app._store.close()
    asyncio.run(scenario())


def test_cancel_approval_under_inspector_does_not_leave_orphan_modal(tmp_path):
    async def scenario():
        app, workspace = _make_app(tmp_path, _create_file_script())
        try:
            async with app.run_test(size=(80, 24)) as pilot:
                prompt = app.query_one("#prompt", PromptArea)
                app.post_message(PromptArea.Submitted("write", prompt))
                await _wait_for_modal(app)
                await pilot.press("ctrl+i")
                await pilot.press("ctrl+c")
                await _wait_turn_done(app)
                await pilot.pause()
                assert not any(isinstance(s, ApprovalModal) for s in app.screen_stack)
                assert not (workspace / "new.txt").exists()
        finally:
            app._store.close()
    asyncio.run(scenario())
