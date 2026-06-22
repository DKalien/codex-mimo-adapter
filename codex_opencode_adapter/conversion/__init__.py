"""Responses ↔ Chat Completions conversion layer.

The modules in this package intentionally keep protocol conversion separate from
HTTP serving, upstream I/O, and persistence. The shape follows the same concern
split as mature Codex bridge implementations: tool context, request transform,
non-stream response transform, and streaming response transform.
"""
