// One program draws every piece: pass 0 fills (media-textured triangles),
// pass 1 outlines (line segments). Per-piece state lives in an RGBA32F texture,
// two texels per piece, STATE_PER_ROW pieces per texture row:
//   texel 0 = (x, y, quarter turns 0..3, flags)   texel 1 = (depth 0..1, 0, 0, 0)

export const STATE_PER_ROW = 256;

/** Piece flag bits (texel 0 .w). */
export const FLAG_HELD = 1;
export const FLAG_PLACED = 2;
export const FLAG_SELECTED = 4;
/** Internal: the board backdrop quad that rides along in the piece batch. */
export const FLAG_BACKDROP = 8;

export const VERT = `#version 300 es
precision highp float;
precision highp int;
precision highp sampler2D;
layout(location = 0) in vec2 a_pos;
layout(location = 1) in vec2 a_uv;
layout(location = 2) in uint a_piece;
uniform sampler2D u_state;
uniform vec3 u_view;      // scale, offset x, offset y (CSS px)
uniform vec2 u_viewSize;  // CSS px
uniform vec2 u_cell;      // cell width, height (source px)
uniform vec4 u_crop;      // media uv rect: u0, v0, u1, v1
uniform int u_pass;
out vec2 v_uv;
flat out int v_flags;
void main() {
  int idx = int(a_piece);
  ivec2 at = ivec2((idx % ${STATE_PER_ROW}) * 2, idx / ${STATE_PER_ROW});
  vec4 s0 = texelFetch(u_state, at, 0);
  vec4 s1 = texelFetch(u_state, at + ivec2(1, 0), 0);
  vec2 c = u_cell * 0.5;
  vec2 p = a_pos - c;
  int rot = int(s0.z + 0.5) & 3;
  if (rot == 1) p = vec2(-p.y, p.x);
  else if (rot == 2) p = -p;
  else if (rot == 3) p = vec2(p.y, -p.x);
  vec2 t = s0.xy + c + p;
  vec2 screen = t * u_view.x + u_view.yz;
  vec2 clip = screen / u_viewSize * 2.0 - 1.0;
  // Higher depth value = nearer. Outlines sit a hair in front of their own fill.
  float z = 0.999 - 1.99 * s1.x - (u_pass == 1 ? 0.00002 : 0.0);
  gl_Position = vec4(clip.x, -clip.y, z, 1.0);
  v_uv = mix(u_crop.xy, u_crop.zw, a_uv);
  v_flags = int(s0.w + 0.5);
}
`;

export const FRAG = `#version 300 es
precision mediump float;
precision highp int;
uniform sampler2D u_media;
uniform int u_pass;
uniform int u_hasMedia;
in vec2 v_uv;
flat in int v_flags;
out vec4 o;
void main() {
  if (u_pass == 0) {
    if ((v_flags & ${FLAG_BACKDROP}) != 0) { o = vec4(0.10, 0.10, 0.12, 1.0); return; }
    vec3 c = u_hasMedia == 1 ? texture(u_media, v_uv).rgb : vec3(0.45, 0.45, 0.5);
    if ((v_flags & ${FLAG_HELD}) != 0) c = c * 1.06 + 0.04;
    if ((v_flags & ${FLAG_SELECTED}) != 0) c = mix(c, vec3(1.0, 0.85, 0.2), 0.2);
    o = vec4(c, 1.0);
  } else {
    if ((v_flags & ${FLAG_SELECTED}) != 0) o = vec4(1.0, 0.85, 0.2, 1.0);
    else if ((v_flags & ${FLAG_HELD}) != 0) o = vec4(1.0, 1.0, 1.0, 0.9);
    else if ((v_flags & ${FLAG_PLACED}) != 0) o = vec4(0.0, 0.0, 0.0, 0.12);
    else o = vec4(0.0, 0.0, 0.0, 0.5);
  }
}
`;
