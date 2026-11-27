// The C face of LibRaw that lcc-libraw calls: open a file held in memory, unpack its raw data, and report the
// mosaic, the colour filter layout, the levels and the camera's colour numbers in one plain struct. Nothing here
// processes the image (no black subtraction, scaling, white balance or demosaic): the app does that in Rust.
//
// Calls LibRaw (https://github.com/LibRaw/LibRaw, LGPL-2.1 or CDDL-1.0, used under the LGPL-2.1) through its
// public C++ API only. Every entry point catches C++ exceptions, so none crosses into Rust.

#include "libraw/libraw.h"

#include <cstdint>
#include <cstdio>
#include <cstring>
#include <new>

// LibRaw with the one internal number the shim needs: a Fujifilm SuperCCD file's rotated layout width.
class Reader : public LibRaw {
public:
  Reader() : LibRaw(0) {}
  unsigned rotated_width() { return libraw_internal_data.internal_output_params.fuji_width; }
};

extern "C" {

// Keep in step with `Info` in src/ffi.rs.
struct lcc_libraw_info {
  uint32_t raw_width, raw_height;
  uint32_t left_margin, top_margin, width, height;
  uint32_t raw_pitch; // bytes per row of raw_image
  uint32_t filters;
  int32_t colors;
  int32_t is_mosaic; // raw_image holds one sample per pixel
  uint32_t fuji_width;
  uint8_t pattern[48][48]; // COLOR(row, col) at image-area coordinates
  uint32_t black;
  uint32_t cblack[6 + 256];
  uint32_t maximum;
  uint32_t raw_bps;
  float cam_mul[4];
  float cam_xyz[4][3];
};

static void say(char *err, size_t len, const char *what) {
  if (err && len > 0) {
    std::snprintf(err, len, "%s", what ? what : "LibRaw failed");
  }
}

const char *lcc_libraw_version(void) { return LibRaw::version(); }

void lcc_libraw_close(void *handle) {
  try {
    delete static_cast<Reader *>(handle);
  } catch (...) {
  }
}

void *lcc_libraw_open(const uint8_t *buf, size_t len, lcc_libraw_info *out, char *err, size_t err_len) {
  Reader *lr = nullptr;
  try {
    if (!buf || !out || len == 0) {
      say(err, err_len, "empty input");
      return nullptr;
    }
    std::memset(out, 0, sizeof(*out));
    lr = new (std::nothrow) Reader();
    if (!lr) {
      say(err, err_len, "out of memory");
      return nullptr;
    }
    int rc = lr->open_buffer(buf, len);
    if (rc == LIBRAW_SUCCESS) {
      rc = lr->unpack();
    }
    if (rc != LIBRAW_SUCCESS) {
      say(err, err_len, libraw_strerror(rc));
      delete lr;
      return nullptr;
    }
    const libraw_data_t &d = lr->imgdata;
    out->raw_width = d.sizes.raw_width;
    out->raw_height = d.sizes.raw_height;
    out->left_margin = d.sizes.left_margin;
    out->top_margin = d.sizes.top_margin;
    out->width = d.sizes.width;
    out->height = d.sizes.height;
    out->raw_pitch = d.sizes.raw_pitch;
    out->filters = d.idata.filters;
    out->colors = d.idata.colors;
    out->is_mosaic = d.rawdata.raw_image != nullptr;
    out->fuji_width = lr->rotated_width();
    for (int r = 0; r < 48; r++) {
      for (int c = 0; c < 48; c++) {
        int v = lr->COLOR(r, c);
        out->pattern[r][c] = (uint8_t)(v < 0 || v > 255 ? 255 : v);
      }
    }
    out->black = d.color.black;
    for (int i = 0; i < 6 + 256; i++) {
      out->cblack[i] = d.color.cblack[i];
    }
    out->maximum = d.color.maximum;
    out->raw_bps = d.color.raw_bps;
    for (int i = 0; i < 4; i++) {
      out->cam_mul[i] = d.color.cam_mul[i];
      for (int j = 0; j < 3; j++) {
        out->cam_xyz[i][j] = d.color.cam_xyz[i][j];
      }
    }
    return lr;
  } catch (...) {
    say(err, err_len, "LibRaw raised an exception");
    delete lr;
    return nullptr;
  }
}

// Copies the mosaic, `raw_height` rows of `raw_width` samples, into `dst` (`dst_len` samples). 0 on success.
int lcc_libraw_copy(void *handle, uint16_t *dst, size_t dst_len) {
  try {
    Reader *lr = static_cast<Reader *>(handle);
    if (!lr || !dst) {
      return 1;
    }
    const libraw_data_t &d = lr->imgdata;
    const uint16_t *src = d.rawdata.raw_image;
    size_t w = d.sizes.raw_width, h = d.sizes.raw_height, pitch = d.sizes.raw_pitch / 2;
    if (!src || pitch < w || dst_len / (w ? w : 1) < h || w * h > dst_len) {
      return 2;
    }
    for (size_t row = 0; row < h; row++) {
      std::memcpy(dst + row * w, src + row * pitch, w * sizeof(uint16_t));
    }
    return 0;
  } catch (...) {
    return 3;
  }
}

} // extern "C"
