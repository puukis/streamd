# Vendored NVENC Header

`streamd-server` vendors `include/ffnvcodec/nvEncodeAPI.h` for build-time bindgen.
The file was copied from `C:/nvenc/nvEncodeAPI.h` on 2026-04-02 and reports
`NVENCAPI_MAJOR_VERSION 13` / `NVENCAPI_MINOR_VERSION 0`.

The upstream copyright and permission notice is preserved verbatim at the top of
`nvEncodeAPI.h`. Only this header is vendored here. NVIDIA runtime libraries are
not redistributed by this repository and must still come from the installed
driver stack.

The directory layout mirrors `nv-codec-headers` so `NVENC_INCLUDE_DIR` can point
at either this vendored `include/` directory or an externally installed one.
