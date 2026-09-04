"""`python -m ml_pipeline` — the service on localhost.

Started by `just ml-pipeline-serve`. It says where the notebooks are and what
the security posture is, because both are things somebody running this needs to
have read once.
"""

from __future__ import annotations

import uvicorn

from ml_pipeline.config import Config
from ml_pipeline.log import configure, logger
from ml_pipeline.service import APP_PATH, create_app

log = logger(__name__)


def main() -> None:
    configure()
    config = Config.from_env()
    app = create_app(config)
    log.info(
        "service.start",
        notebooks=str(config.notebooks),
        staged_rows=str(config.data),
        url=f"http://{config.host}:{config.port}{APP_PATH}/<notebook>/",
        max_rows=config.max_rows,
        timeout_seconds=config.timeout_seconds,
    )
    log.warning("service.unsandboxed", detail="runs notebook code with no sandbox — localhost only")
    # marimo's app host and this service share a process, and uvicorn's own
    # access log would be a second format in the same stream.
    uvicorn.run(app, host=config.host, port=config.port, log_level="warning")


if __name__ == "__main__":
    main()
