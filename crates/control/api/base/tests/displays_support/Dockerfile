# The disposable desktop runs the shipped display server and adds the desktop-sharing program.
FROM orch-display-fixture-base
USER root
RUN apt-get update && apt-get install -y --no-install-recommends x11vnc && rm -rf /var/lib/apt/lists/*
USER agent
