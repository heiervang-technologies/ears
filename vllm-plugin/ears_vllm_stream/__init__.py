"""ears stream protocol (docs/STREAM_PROTOCOL.md) as a vLLM endpoint plugin."""

__all__ = ["EarsStreamPlugin"]


def __getattr__(name):
    # Lazy, so `ears_vllm_stream.protocol` imports without fastapi installed.
    if name == "EarsStreamPlugin":
        from .plugin import EarsStreamPlugin

        return EarsStreamPlugin
    raise AttributeError(name)
