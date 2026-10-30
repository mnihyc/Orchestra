Release archives carry one generated `THIRD_PARTY_NOTICES` file for the components they ship.
At packaging time it is built offline from target-filtered Rust metadata, the npm packages emitted by the production web build and their licence and notice files, using standard SPDX texts with available copyright lines when files are missing.
This repository keeps its own `LICENSE` and the original terms of vendored code in `licenses/novnc-1.7.0/` and `licenses/mammoth-1.13.0/`.
