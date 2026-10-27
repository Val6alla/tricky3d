import { CanvasTexture, SRGBColorSpace, Sprite, SpriteMaterial } from "three";

const GLYPH_PX = 64;

/**
 * Text drawn once into a canvas texture. The glyphs are a white mask, so SpriteMaterial.color
 * sets the visible colour and a theme change needs no redraw.
 */
export interface TextLabel {
  material: SpriteMaterial;
  /** Texture width / height. */
  aspect: number;
}

export function makeTextLabel(text: string, font: string): TextLabel {
  const canvas = document.createElement("canvas");
  const ctx = canvas.getContext("2d");
  const spec = `600 ${GLYPH_PX}px ${font}`;
  const pad = Math.round(GLYPH_PX * 0.15);
  let textWidth = GLYPH_PX;
  if (ctx) {
    ctx.font = spec;
    textWidth = Math.ceil(ctx.measureText(text).width);
  }
  canvas.width = textWidth + pad * 2;
  canvas.height = Math.round(GLYPH_PX * 1.2);
  if (ctx) {
    ctx.font = spec; // resizing the canvas reset the context state
    ctx.fillStyle = "#fff"; // mask only; the material colour tints it
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillText(text, canvas.width / 2, canvas.height / 2);
  }
  const map = new CanvasTexture(canvas);
  map.colorSpace = SRGBColorSpace;
  const material = new SpriteMaterial({ map, transparent: true, depthWrite: false });
  return { material, aspect: canvas.width / canvas.height };
}

/** A camera-facing sprite showing `label`, `height` world units tall. */
export function labelSprite(label: TextLabel, height: number): Sprite {
  const sprite = new Sprite(label.material);
  sprite.scale.set(height * label.aspect, height, 1);
  return sprite;
}

export function disposeLabel(label: TextLabel) {
  label.material.map?.dispose();
  label.material.dispose();
}
