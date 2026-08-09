/**
 * Pure classification of a markdown media URL: whether it should render as a
 * video.
 *
 * `isVideoMedia` decides the render path (video player vs. image block). Kept
 * DOM-free so the branch logic is unit-testable without a webview.
 *
 * Media download eligibility was removed in the native cutover — there is no
 * remote media transport, so no Download action is offered for any media URL.
 */

/** Legacy video extensions, used only when an imeta MIME type is absent. */
const VIDEO_EXTENSIONS = ["mp4", "webm", "mov"] as const;

/** The lowercased path extension of a URL, ignoring query strings and hashes. */
function urlPathExtension(src: string): string | undefined {
  let pathname: string;
  try {
    pathname = new URL(src).pathname;
  } catch {
    // Relative or malformed URL — strip query/hash by hand.
    pathname = src.split(/[?#]/, 1)[0];
  }
  const lastDot = pathname.lastIndexOf(".");
  if (lastDot < 0 || lastDot === pathname.length - 1) return undefined;
  return pathname.slice(lastDot + 1).toLowerCase();
}

/**
 * Whether `src` should render as a video.
 *
 * The imeta MIME type is authoritative when present (uploads tag every
 * attachment with `m`): a `video/*` MIME renders as video, and any other MIME
 * renders as an image regardless of the URL extension. Only when the MIME is
 * absent (legacy events that predate the tag) do we fall back to a path
 * extension check.
 */
export function isVideoMedia(src: string, imetaMime?: string): boolean {
  if (imetaMime) return imetaMime.toLowerCase().startsWith("video/");
  const ext = urlPathExtension(src);
  return (
    ext !== undefined && (VIDEO_EXTENSIONS as readonly string[]).includes(ext)
  );
}
