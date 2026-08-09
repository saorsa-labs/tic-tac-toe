type SnapshotAvatarPngDependencies = {
  createCanvas?: () => HTMLCanvasElement;
  createImage?: () => HTMLImageElement;
};

/**
 * Resolve an avatar to PNG data for the image body of a snapshot PNG.
 *
 * The original avatar URL remains in the manifest so imports preserve the
 * editable source; this only supplies a renderable card thumbnail. Only
 * inline SVG data URLs can be rasterized locally — remote HTTPS avatars have
 * no fetch transport and are left for the manifest URL to render.
 */
export async function resolveSnapshotAvatarPng(
  avatarUrl: string | null | undefined,
  dependencies: SnapshotAvatarPngDependencies = {},
): Promise<string | undefined> {
  const url = avatarUrl?.trim();
  if (!url) return undefined;

  if (!isSvgDataUrl(url)) return undefined;

  return rasterizeSvg(url, dependencies);
}

function isSvgDataUrl(url: string) {
  return /^data:image\/svg\+xml(?:;[^,]*)?,/i.test(url);
}

async function rasterizeSvg(
  svgDataUrl: string,
  dependencies: SnapshotAvatarPngDependencies,
): Promise<string | undefined> {
  try {
    const image = (dependencies.createImage ?? (() => new Image()))();
    image.src = squareEmojiAvatarBackground(svgDataUrl);
    await image.decode();

    const canvas = (
      dependencies.createCanvas ?? (() => document.createElement("canvas"))
    )();
    canvas.width = 512;
    canvas.height = 512;
    const context = canvas.getContext("2d");
    if (!context) return undefined;
    context.drawImage(image, 0, 0, canvas.width, canvas.height);
    return canvas.toDataURL("image/png");
  } catch {
    return undefined;
  }
}

/**
 * Emoji avatars use a circular SVG background in profile surfaces. Snapshot
 * attachments already clip artwork to a rounded-square media slot, so remove
 * that source-level circle before rasterizing to let the artwork fill the slot.
 */
function squareEmojiAvatarBackground(svgDataUrl: string) {
  const commaIndex = svgDataUrl.indexOf(",");
  if (
    commaIndex === -1 ||
    svgDataUrl.slice(0, commaIndex).includes(";base64")
  ) {
    return svgDataUrl;
  }

  try {
    const prefix = svgDataUrl.slice(0, commaIndex + 1);
    const svg = decodeURIComponent(svgDataUrl.slice(commaIndex + 1));
    const squaredSvg = svg.replace(
      /(<rect\b[^>]*\bwidth="512"[^>]*\bheight="512"[^>]*?)\s+rx="256"/u,
      "$1",
    );

    return squaredSvg === svg
      ? svgDataUrl
      : `${prefix}${encodeURIComponent(squaredSvg)}`;
  } catch {
    return svgDataUrl;
  }
}
