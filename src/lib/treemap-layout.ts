/**
 * Squarified treemap layout (Bruls, Huizing & van Wijk, 2000).
 *
 * Pure and dependency-free: given weighted items and a pixel rectangle it
 * returns one rectangle per item, with areas proportional to the weights and
 * aspect ratios kept as close to 1 as possible. Items too small to render can
 * be pre-aggregated by the caller into a single "other" item.
 */

export interface TreemapItem {
  id: string;
  /** Non-negative weight; zero/negative items are skipped. */
  weight: number;
}

export interface TreemapRect {
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface TreemapBox {
  x: number;
  y: number;
  w: number;
  h: number;
}

interface Cell {
  id: string;
  area: number;
}

/**
 * Lay out `items` inside `box`. Returns rectangles in no particular order.
 * When every weight is zero (or the box has no area) an empty list is
 * returned.
 */
export function layoutTreemap(
  items: readonly TreemapItem[],
  box: TreemapBox
): TreemapRect[] {
  if (!(box.w > 0) || !(box.h > 0)) return [];

  const total = items.reduce((sum, item) => sum + Math.max(0, item.weight), 0);
  if (!(total > 0)) return [];

  const areaScale = (box.w * box.h) / total;
  const cells: Cell[] = items
    .filter((item) => item.weight > 0)
    .map((item) => ({ id: item.id, area: item.weight * areaScale }))
    .sort((a, b) => b.area - a.area);

  const rects: TreemapRect[] = [];
  const current: TreemapBox = { ...box };
  let row: Cell[] = [];

  const shortestSide = () => Math.min(current.w, current.h);

  const emitRow = (placed: Cell[]) => {
    if (placed.length === 0) return;
    const rowArea = placed.reduce((sum, cell) => sum + cell.area, 0);
    // Rows run along the long axis; the strip fills the short edge.
    if (current.w <= current.h) {
      const thickness = clamp(rowArea / current.w, 0, current.h);
      let cursorX = current.x;
      for (const cell of placed) {
        const length = clamp(cell.area / thickness, 0, current.w);
        rects.push({ id: cell.id, x: cursorX, y: current.y, w: length, h: thickness });
        cursorX += length;
      }
      current.y += thickness;
      current.h -= thickness;
    } else {
      const thickness = clamp(rowArea / current.h, 0, current.w);
      let cursorY = current.y;
      for (const cell of placed) {
        const length = clamp(cell.area / thickness, 0, current.h);
        rects.push({ id: cell.id, x: current.x, y: cursorY, w: thickness, h: length });
        cursorY += length;
      }
      current.x += thickness;
      current.w -= thickness;
    }
  };

  for (const cell of cells) {
    if (row.length > 0 && worstRatio(row, shortestSide()) < worstRatio([...row, cell], shortestSide())) {
      emitRow(row);
      row = [];
    }
    row.push(cell);
  }
  emitRow(row);

  return rects;
}

/**
 * Worst (largest) aspect ratio among the rectangles a row would produce if
 * laid along an edge of length `side`. Values close to 1 are good.
 */
function worstRatio(row: Cell[], side: number): number {
  const sum = row.reduce((total, cell) => total + cell.area, 0);
  const max = Math.max(...row.map((cell) => cell.area));
  const min = Math.min(...row.map((cell) => cell.area));
  const sideSq = side * side;
  const sumSq = sum * sum;
  return Math.max((sideSq * max) / sumSq, sumSq / (sideSq * min));
}

function clamp(value: number, min: number, max: number): number {
  if (value < min) return min;
  if (value > max) return max;
  return value;
}
