# AuroraLynx source provenance

AuroraLynx is the Android recovery maintained for AuroraAOSP. This source snapshot derives from [kirklin/skyward](https://github.com/kirklin/skyward) at commit [43ec6da19402cf344b966bbb8c85c04fd2ffdcad](https://github.com/kirklin/skyward/commit/43ec6da19402cf344b966bbb8c85c04fd2ffdcad). The archived upstream README (UPSTREAM-README.md and UPSTREAM-README.zh-CN.md), project license and source copyright notices accompany the snapshot.

The Android product integration and replayable patch stack live in AuroraAOSP. This repository tracks the resulting recovery source. Follow the AuroraAOSP product build instructions for an actual device recovery image.

## Host validation

```sh
cargo test --locked -p skyward-ui -p skyward-preview
cargo run --locked -p skyward-preview -- /tmp/auroralynx-preview --style-matrix --lang zh-Hans
```
