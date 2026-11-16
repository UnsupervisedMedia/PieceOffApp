/** Table (source-pixel) <-> screen (CSS pixel) mapping: `screen = table * scale + offset`.
 *  The board renderer turns it into clip space; input uses it for hit-testing. */
export class Camera {
  scale = 1;
  ox = 0;
  oy = 0;
  minScale = 0.02;
  maxScale = 64;

  constructor(
    public viewW: number,
    public viewH: number,
  ) {}

  /** New view size; keeps the table point under the view centre fixed. */
  resize(viewW: number, viewH: number): void {
    const [cx, cy] = this.toTable(this.viewW / 2, this.viewH / 2);
    this.viewW = viewW;
    this.viewH = viewH;
    this.ox = viewW / 2 - cx * this.scale;
    this.oy = viewH / 2 - cy * this.scale;
  }

  toTable(sx: number, sy: number): [number, number] {
    return [(sx - this.ox) / this.scale, (sy - this.oy) / this.scale];
  }

  toScreen(tx: number, ty: number): [number, number] {
    return [tx * this.scale + this.ox, ty * this.scale + this.oy];
  }

  panBy(dx: number, dy: number): void {
    this.ox += dx;
    this.oy += dy;
  }

  /** Zoom by `factor` about screen point (sx, sy), which stays put. */
  zoomAt(sx: number, sy: number, factor: number): void {
    if (!(factor > 0) || !Number.isFinite(factor)) return;
    const [tx, ty] = this.toTable(sx, sy);
    this.scale = Math.min(this.maxScale, Math.max(this.minScale, this.scale * factor));
    this.ox = sx - tx * this.scale;
    this.oy = sy - ty * this.scale;
  }

  /** Show the table rectangle (minX, minY, tableW, tableH) whole and centred. */
  fit(tableW: number, tableH: number, minX = 0, minY = 0): void {
    if (tableW <= 0 || tableH <= 0) return;
    this.scale = Math.min(this.viewW / tableW, this.viewH / tableH);
    this.ox = this.viewW / 2 - (minX + tableW / 2) * this.scale;
    this.oy = this.viewH / 2 - (minY + tableH / 2) * this.scale;
  }
}
