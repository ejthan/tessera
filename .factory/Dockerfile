# Sandbox image for the factory (loopia-factory). Based on the factory's default image (node:24-bookworm, the `agent`
# user aligned to the host UID/GID, Claude Code, ctx7) plus Rust, the GPUI build packages and a typesense-server
# binary, because the sandbox has no Docker. Never put secrets here.
FROM node:24-bookworm

# GPUI packages from .github/workflows/ci.yml (LINUX_DEPS), plus xvfb, a software Vulkan driver, ImageMagick,
# ffmpeg and xdotool so the agent can start the app headless and capture evidence.
RUN apt-get update && apt-get install -y \
  git curl jq \
  gcc g++ clang libfontconfig-dev libwayland-dev libwebkit2gtk-4.1-dev \
  libxkbcommon-x11-dev libx11-xcb-dev libssl-dev libzstd-dev \
  vulkan-validationlayers libvulkan1 \
  xvfb xauth mesa-vulkan-drivers imagemagick ffmpeg xdotool \
  && rm -rf /var/lib/apt/lists/*

# Keep in step with the version the CI integration job tests last.
ARG TYPESENSE_VERSION=30.2
RUN curl -fsSL "https://dl.typesense.org/releases/${TYPESENSE_VERSION}/typesense-server-${TYPESENSE_VERSION}-linux-$(dpkg --print-architecture).tar.gz" \
  | tar -xz -C /usr/local/bin typesense-server \
  && test -x /usr/local/bin/typesense-server

ENV CTX7_TELEMETRY_DISABLED=1
RUN npm i -g ctx7@0.9.0

ARG AGENT_UID=1000
ARG AGENT_GID=1000

# Rename the base image's "node" user to "agent" and align UID/GID.
RUN groupmod -o -g $AGENT_GID node && usermod -o -u $AGENT_UID -g $AGENT_GID -d /home/agent -m -l agent node
USER ${AGENT_UID}:${AGENT_GID}

RUN curl -fsSL https://claude.ai/install.sh | bash

# Rust in the agent's home, so ~/.cargo/registry/src/* (CLAUDE.md rule 1) holds the pinned crate sources.
# The toolchain matches rust-toolchain.toml; if that moves on, rustup installs the new one on the first cargo call.
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.99.1 -c rustfmt -c clippy
ENV PATH="/home/agent/.cargo/bin:/home/agent/.local/bin:$PATH"

# The factory caps sandboxes at 4 GB: fewer parallel rustc jobs and no debug info keep GPUI builds and links below it.
ENV CARGO_BUILD_JOBS=4 \
  CARGO_PROFILE_DEV_DEBUG=0 \
  CARGO_PROFILE_TEST_DEBUG=0

WORKDIR /home/agent

ENTRYPOINT ["sleep", "infinity"]
