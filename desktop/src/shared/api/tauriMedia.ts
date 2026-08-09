import { invokeTauri } from "./tauri";

/** Write text through the native clipboard after an asynchronous workflow. */
export async function copyTextToSystemClipboard(
  text: string,
  html?: string,
): Promise<void> {
  await invokeTauri("copy_text_to_clipboard", { html, text });
}
