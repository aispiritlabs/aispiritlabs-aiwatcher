from __future__ import annotations

from aiwatcher_agentic.workflow.errors import (
    AgenticError,
    ConcurrencyConflictError,
    IllegalStateError,
    NotFoundError,
    StepLimitExceeded,
    ValidationError,
)


class TestErrorHierarchy:
    def test_all_errors_inherit_from_agentic_error(self) -> None:
        errors: list[type[AgenticError]] = [
            ValidationError,
            IllegalStateError,
            NotFoundError,
            ConcurrencyConflictError,
            StepLimitExceeded,
        ]
        for error_cls in errors:
            assert issubclass(error_cls, AgenticError)
            assert issubclass(error_cls, RuntimeError)

    def test_error_codes(self) -> None:
        assert ValidationError.error_code == 400
        assert IllegalStateError.error_code == 403
        assert NotFoundError.error_code == 404
        assert ConcurrencyConflictError.error_code == 412
        assert StepLimitExceeded.error_code == 429
        assert AgenticError.error_code == 500

    def test_concurrency_conflict_error_attributes(self) -> None:
        error = ConcurrencyConflictError("cart-1", expected=1, current=2)
        assert error.stream_name == "cart-1"
        assert error.expected == 1
        assert error.current == 2
        assert "cart-1" in str(error)

    def test_step_limit_exceeded_attributes(self) -> None:
        error = StepLimitExceeded(max_steps=50)
        assert error.max_steps == 50
        assert "50" in str(error)

    def test_isinstance_catch_all(self) -> None:
        """AgenticError catches all domain errors."""
        errors = [
            ValidationError("bad input"),
            IllegalStateError("wrong state"),
            NotFoundError("missing"),
            ConcurrencyConflictError("s", 1, 2),
            StepLimitExceeded(10),
        ]
        for error in errors:
            assert isinstance(error, AgenticError)
