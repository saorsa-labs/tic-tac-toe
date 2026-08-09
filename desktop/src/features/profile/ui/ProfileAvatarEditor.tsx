import emojiData from "@emoji-mart/data";
import Picker from "@emoji-mart/react";
import { Link2 } from "lucide-react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import * as React from "react";

import { AvatarCustomColorPanel } from "@/features/profile/ui/AvatarCustomColorPanel";
import { ProfileAvatarModeTabs } from "@/features/profile/ui/ProfileAvatarModeTabs";
import { useAvatarSelection } from "@/features/profile/avatarPresentationStore";
import { cn } from "@/shared/lib/cn";
import { Button } from "@/shared/ui/button";
import { useEmojiBurst } from "@/shared/ui/EmojiBurstProvider";
import { Spinner } from "@/shared/ui/spinner";
import {
  DONE_BUTTON_CONTENT_TRANSITION,
  DONE_BUTTON_SHELL_TRANSITION,
} from "./ProfileAvatarEditor.helpers";
import {
  AVATAR_COLORS,
  AVATAR_COLOR_SWATCHES,
  CUSTOM_AVATAR_COLOR_SWATCH,
  DEFAULT_CUSTOM_HUE,
  DEFAULT_CUSTOM_SATURATION,
  DEFAULT_CUSTOM_VALUE,
  DEFAULT_EMOJI_AVATAR_COLOR,
  EMOJI_MART_CATEGORIES,
  type AvatarColorSwatch,
  contrastColorForBackground,
  emojiAvatarDataUrl,
  hexToHsv,
  hsvToHex,
  normalizeHue,
  parseEmojiAvatarDataUrl,
  useEmojiMartStyles,
  useEmojiMartThemeVars,
} from "./ProfileAvatarEditor.utils";
export { parseEmojiAvatarDataUrl } from "./ProfileAvatarEditor.utils";
export type { AvatarMode } from "./ProfileAvatarEditor.types";
import type {
  AvatarMode,
  ProfileAvatarEditorProps,
} from "./ProfileAvatarEditor.types";

const INITIAL_EMOJI_AVATAR_COLORS = AVATAR_COLORS.filter(
  (color) => color !== DEFAULT_EMOJI_AVATAR_COLOR,
);

function randomInitialEmojiAvatarColor() {
  const colors =
    INITIAL_EMOJI_AVATAR_COLORS.length > 0
      ? INITIAL_EMOJI_AVATAR_COLORS
      : AVATAR_COLORS;
  return (
    colors[Math.floor(Math.random() * colors.length)] ??
    DEFAULT_EMOJI_AVATAR_COLOR
  );
}

export function ProfileAvatarEditor({
  avatarUrl,
  donePending = false,
  emojiPickerTheme = "dark",
  emojiPickerThemeVars,
  onCustomColorPickerOpenChange,
  onEmojiAvatarChange,
  onModeChange,
  onUrlChange,
  onDone,
  showEmojiColorControlsWhenEmpty = false,
  disabled,
  testIdPrefix = "profile-avatar",
  modeTabsContainer,
  presentation = "default",
}: ProfileAvatarEditorProps) {
  const { burstEmoji } = useEmojiBurst();
  const shouldReduceMotion = useReducedMotion();
  const initialEmojiAvatar = React.useMemo(
    () => parseEmojiAvatarDataUrl(avatarUrl),
    [avatarUrl],
  );
  const [mode, setMode] = React.useState<AvatarMode>("image");
  const [urlDraft, setUrlDraft] = React.useState("");
  const [selectedEmoji, setSelectedEmoji] = React.useState<string | null>(
    () => initialEmojiAvatar?.emoji ?? null,
  );
  const [selectedColor, setSelectedColor] = React.useState(
    () => initialEmojiAvatar?.color ?? DEFAULT_EMOJI_AVATAR_COLOR,
  );
  const [customHue, setCustomHue] = React.useState(DEFAULT_CUSTOM_HUE);
  const [customSaturation, setCustomSaturation] = React.useState(
    DEFAULT_CUSTOM_SATURATION,
  );
  const [customValue, setCustomValue] = React.useState(DEFAULT_CUSTOM_VALUE);
  const [isCustomColorPickerOpen, setIsCustomColorPickerOpen] =
    React.useState(false);
  const emojiPickerContainerRef = React.useRef<HTMLDivElement | null>(null);
  const modeContentRef = React.useRef<HTMLDivElement | null>(null);
  const isUrlInputFocusedRef = React.useRef(false);
  const hasUserEditedUrlDraftRef = React.useRef(false);
  const [modeContentHeight, setModeContentHeight] = React.useState<
    number | null
  >(null);
  const documentEmojiMartThemeVars = useEmojiMartThemeVars();
  const emojiMartThemeVars = React.useMemo(
    () =>
      ({
        ...(emojiPickerThemeVars ?? documentEmojiMartThemeVars),
        ...(presentation === "onboarding-modal"
          ? {
              "--buzz-emoji-picker-category-icon-size": "18px",
              "--buzz-emoji-picker-fade-height": "56px",
              "--buzz-emoji-picker-fade-opacity": "1",
              "--buzz-emoji-picker-nav-button-size": "32px",
              "--buzz-emoji-picker-nav-padding-x": "12px",
              "--buzz-emoji-picker-padding": "10px",
              "--buzz-emoji-picker-scroll-padding-top": "18px",
            }
          : null),
      }) as React.CSSProperties,
    [documentEmojiMartThemeVars, emojiPickerThemeVars, presentation],
  );
  const customColorDraft = React.useMemo(
    () => hsvToHex(customHue, customSaturation, customValue),
    [customHue, customSaturation, customValue],
  );
  const isOnboardingModal = presentation === "onboarding-modal";
  const shouldShowColorControls =
    mode === "emoji" &&
    (selectedEmoji !== null || showEmojiColorControlsWhenEmpty);
  const isCustomColorPickerVisible =
    isCustomColorPickerOpen && shouldShowColorControls;
  const updateMode = React.useCallback(
    (nextMode: AvatarMode) => {
      if (mode === nextMode) {
        return;
      }

      setMode(nextMode);
      onModeChange?.(nextMode);
    },
    [mode, onModeChange],
  );
  const setAvatar = useAvatarSelection(avatarUrl, onUrlChange);
  const isInputDisabled = Boolean(disabled);
  const isDoneButtonPending = Boolean(donePending);
  const handleDoneClick = React.useCallback(() => {
    onDone?.();
  }, [onDone]);

  useEmojiMartStyles(emojiPickerContainerRef, mode === "emoji");

  React.useEffect(() => {
    if (mode !== "emoji") return;

    let animationFrame = 0;
    let observer: MutationObserver | null = null;
    const syncSelectedEmojiButton = () => {
      const shadowRoot =
        emojiPickerContainerRef.current?.querySelector(
          "em-emoji-picker",
        )?.shadowRoot;
      if (!shadowRoot) {
        animationFrame = window.requestAnimationFrame(syncSelectedEmojiButton);
        return;
      }

      shadowRoot
        .querySelectorAll('button[data-buzz-selected="true"]')
        .forEach((button) => {
          button.removeAttribute("data-buzz-selected");
        });
      if (selectedEmoji) {
        shadowRoot.querySelectorAll(".category button").forEach((button) => {
          if (button.getAttribute("aria-label") === selectedEmoji) {
            button.setAttribute("data-buzz-selected", "true");
          }
        });
      }

      observer ??= new MutationObserver(syncSelectedEmojiButton);
      observer.observe(shadowRoot, { childList: true, subtree: true });
    };

    animationFrame = window.requestAnimationFrame(syncSelectedEmojiButton);
    return () => {
      window.cancelAnimationFrame(animationFrame);
      observer?.disconnect();
    };
  }, [mode, selectedEmoji]);

  React.useLayoutEffect(() => {
    const node = modeContentRef.current;
    if (!node) return;

    const updateModeContentHeight = () => {
      setModeContentHeight(node.getBoundingClientRect().height);
    };

    updateModeContentHeight();

    const resizeObserver = new ResizeObserver(updateModeContentHeight);
    resizeObserver.observe(node);

    return () => resizeObserver.disconnect();
  }, []);

  React.useEffect(() => {
    const emojiAvatar = parseEmojiAvatarDataUrl(avatarUrl);
    if (emojiAvatar) {
      setSelectedEmoji(emojiAvatar.emoji);
      setSelectedColor(emojiAvatar.color);
      return;
    }

    setSelectedEmoji(null);
    setSelectedColor(DEFAULT_EMOJI_AVATAR_COLOR);
    setIsCustomColorPickerOpen(false);
  }, [avatarUrl]);

  React.useEffect(() => {
    if (!shouldShowColorControls) setIsCustomColorPickerOpen(false);
  }, [shouldShowColorControls]);

  React.useLayoutEffect(() => {
    onCustomColorPickerOpenChange?.(isCustomColorPickerVisible);

    return () => {
      onCustomColorPickerOpenChange?.(false);
    };
  }, [isCustomColorPickerVisible, onCustomColorPickerOpenChange]);

  React.useEffect(() => {
    if (!isCustomColorPickerOpen || !selectedEmoji) {
      return;
    }

    const nextAvatarUrl = emojiAvatarDataUrl(selectedEmoji, customColorDraft);
    if (avatarUrl === nextAvatarUrl) {
      return;
    }

    setAvatar(nextAvatarUrl);
  }, [
    avatarUrl,
    customColorDraft,
    isCustomColorPickerOpen,
    selectedEmoji,
    setAvatar,
  ]);

  const applyUrl = React.useCallback(() => {
    const nextUrl = urlDraft.trim();
    if (nextUrl.length === 0 || isInputDisabled) {
      hasUserEditedUrlDraftRef.current = false;
      return;
    }

    setAvatar(nextUrl);
    hasUserEditedUrlDraftRef.current = false;
    updateMode("image");
  }, [isInputDisabled, setAvatar, updateMode, urlDraft]);

  const applyEmojiAvatar = React.useCallback(
    (emoji: string, color = selectedColor) => {
      setUrlDraft("");
      hasUserEditedUrlDraftRef.current = false;
      setAvatar(emojiAvatarDataUrl(emoji, color));
      onEmojiAvatarChange?.();
    },
    [onEmojiAvatarChange, selectedColor, setAvatar],
  );

  const openCustomColorPicker = React.useCallback(() => {
    const nextColor = hexToHsv(selectedColor);
    setCustomHue(normalizeHue(nextColor.hue));
    setCustomSaturation(nextColor.saturation);
    setCustomValue(nextColor.value);
    setIsCustomColorPickerOpen(true);
  }, [selectedColor]);

  const commitCustomColor = React.useCallback(() => {
    setSelectedColor(customColorDraft);
    if (selectedEmoji) {
      applyEmojiAvatar(selectedEmoji, customColorDraft);
    }
    setIsCustomColorPickerOpen(false);
  }, [applyEmojiAvatar, customColorDraft, selectedEmoji]);

  const handleColorSelect = React.useCallback(
    (swatch: AvatarColorSwatch) => {
      if (disabled) {
        return;
      }

      if (swatch === CUSTOM_AVATAR_COLOR_SWATCH) {
        if (!selectedEmoji) {
          return;
        }
        openCustomColorPicker();
        return;
      }

      setSelectedColor(swatch);
      if (selectedEmoji) {
        applyEmojiAvatar(selectedEmoji, swatch);
      }
    },
    [applyEmojiAvatar, disabled, openCustomColorPicker, selectedEmoji],
  );

  const shouldShowDoneButton = onDone && !isCustomColorPickerVisible;
  const isDoneButtonDisabled = Boolean(disabled || isDoneButtonPending);
  const modeTabsContent = (
    <ProfileAvatarModeTabs
      disabled={isInputDisabled}
      mode={mode}
      onModeChange={updateMode}
      portalContainer={modeTabsContainer}
      presentation={presentation}
    />
  );

  return (
    <fieldset
      className={cn(
        "mx-auto w-full border-0 p-0 text-sm",
        isOnboardingModal ? "max-w-[456px]" : "max-w-[576px]",
      )}
      data-testid={`${testIdPrefix}-editor`}
      disabled={isInputDisabled}
    >
      <legend className="sr-only">Avatar image picker</legend>
      <div
        className="relative"
        style={
          isOnboardingModal
            ? { minHeight: isCustomColorPickerVisible ? 704 : 454 }
            : undefined
        }
      >
        <div
          className={cn(
            "relative w-full",
            isOnboardingModal ? "flex min-h-[inherit] flex-col" : "grid gap-4",
          )}
        >
          {modeTabsContent}

          <div
            className={cn(
              "transition-[height] duration-[250ms] ease-out",
              isOnboardingModal
                ? cn(
                    "flex min-h-0 flex-1 items-center overflow-visible",
                    shouldShowColorControls && "py-6",
                  )
                : "overflow-hidden",
            )}
            data-testid={`${testIdPrefix}-mode-content-shell`}
            style={
              isOnboardingModal || modeContentHeight === null
                ? undefined
                : { height: modeContentHeight }
            }
          >
            <div
              className={cn("overflow-visible", isOnboardingModal && "w-full")}
              ref={modeContentRef}
            >
              {mode === "image" ? (
                <div className="grid content-start gap-3">
                  <div
                    className={cn(
                      "flex items-center transition-colors duration-[250ms] ease-out",
                      isOnboardingModal
                        ? "h-[52px] rounded-lg border border-[color:rgb(var(--buzz-onboarding-avatar-control-fg)_/_0.45)] bg-transparent px-5 focus-within:border-[rgb(var(--buzz-onboarding-avatar-control-fg))]"
                        : "h-16 gap-3 rounded-xl bg-muted px-5 focus-within:bg-muted/80",
                    )}
                  >
                    {isOnboardingModal ? null : (
                      <Link2 className="h-4 w-4 text-muted-foreground" />
                    )}
                    <input
                      autoCapitalize="none"
                      autoCorrect="off"
                      className={cn(
                        "min-w-0 flex-1 bg-transparent outline-none",
                        isOnboardingModal
                          ? "text-center text-sm font-normal text-foreground placeholder:text-[color:rgb(var(--buzz-onboarding-avatar-control-fg)_/_0.55)]"
                          : "text-sm font-medium text-foreground placeholder:text-muted-foreground",
                      )}
                      data-testid={`${testIdPrefix}-url`}
                      disabled={isInputDisabled}
                      onBlur={() => {
                        isUrlInputFocusedRef.current = false;
                        applyUrl();
                      }}
                      onChange={(event) => {
                        hasUserEditedUrlDraftRef.current = true;
                        setUrlDraft(event.target.value);
                        setAvatar(event.target.value);
                      }}
                      onFocus={() => {
                        isUrlInputFocusedRef.current = true;
                      }}
                      onKeyDown={(event) => {
                        if (event.key === "Enter") {
                          event.preventDefault();
                          applyUrl();
                        }
                      }}
                      placeholder={
                        isOnboardingModal
                          ? "Paste a URL"
                          : "Paste a URL (Slack profile, etc.)"
                      }
                      spellCheck={false}
                      type="url"
                      value={urlDraft}
                    />
                  </div>
                </div>
              ) : (
                <div className="relative grid content-start gap-3">
                  <div
                    className="buzz-emoji-mart relative z-0 h-[316px] overflow-hidden rounded-xl bg-muted transition-colors duration-[250ms] ease-out"
                    ref={emojiPickerContainerRef}
                    style={emojiMartThemeVars}
                  >
                    <Picker
                      categories={EMOJI_MART_CATEGORIES}
                      data={emojiData}
                      dynamicWidth
                      emojiButtonRadius="999px"
                      emojiButtonSize={isOnboardingModal ? 44 : 64}
                      emojiSize={isOnboardingModal ? 28 : 48}
                      icons="outline"
                      navPosition="bottom"
                      onEmojiSelect={(
                        emoji: { native?: string },
                        event?: MouseEvent,
                      ) => {
                        if (isInputDisabled) {
                          return;
                        }
                        if (!emoji.native) {
                          return;
                        }
                        const nextColor =
                          selectedEmoji === null
                            ? randomInitialEmojiAvatarColor()
                            : selectedColor;
                        if (!isOnboardingModal) {
                          burstEmoji(emoji.native, event);
                        }
                        setSelectedEmoji(emoji.native);
                        setSelectedColor(nextColor);
                        applyEmojiAvatar(emoji.native, nextColor);
                      }}
                      previewPosition="none"
                      searchPosition="none"
                      set="native"
                      skinTonePosition="none"
                      theme={emojiPickerTheme}
                    />
                  </div>

                  <div
                    aria-hidden={!shouldShowColorControls}
                    className={cn(
                      showEmojiColorControlsWhenEmpty
                        ? "overflow-hidden"
                        : "origin-top overflow-hidden transition-[max-height,margin,opacity,transform] duration-[250ms] ease-out",
                      shouldShowColorControls
                        ? "mt-3 max-h-64 scale-100 opacity-100"
                        : "mt-0 max-h-0 scale-[0.96] opacity-0",
                    )}
                    data-testid={`${testIdPrefix}-color-grid-shell`}
                    inert={shouldShowColorControls ? undefined : true}
                  >
                    <div
                      className={cn(
                        "grid grid-cols-8 justify-items-center rounded-xl bg-muted transition-colors duration-[250ms] ease-out",
                        isOnboardingModal ? "gap-2 p-3" : "gap-3 p-4",
                      )}
                      data-testid={`${testIdPrefix}-color-grid`}
                    >
                      {AVATAR_COLOR_SWATCHES.map((swatch) => {
                        const isCustomSwatch =
                          swatch === CUSTOM_AVATAR_COLOR_SWATCH;
                        const isSelected = isCustomSwatch
                          ? !AVATAR_COLORS.some(
                              (color) =>
                                color.toUpperCase() ===
                                selectedColor.toUpperCase(),
                            )
                          : swatch.toUpperCase() ===
                            selectedColor.toUpperCase();

                        return (
                          <button
                            aria-label={
                              isCustomSwatch
                                ? selectedEmoji
                                  ? "Choose custom avatar color"
                                  : "Choose an emoji before custom avatar color"
                                : `Use ${swatch} background`
                            }
                            aria-pressed={isSelected}
                            className={cn(
                              "relative scroll-mb-52 rounded-full border border-border transition-transform duration-200 ease-out hover:scale-[1.15] focus-visible:scale-[1.15] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                              isOnboardingModal ? "h-7 w-7" : "h-10 w-10",
                              isCustomSwatch &&
                                !selectedEmoji &&
                                "cursor-not-allowed opacity-45 hover:scale-100 focus-visible:scale-100",
                            )}
                            data-testid={
                              isCustomSwatch
                                ? `${testIdPrefix}-custom-color`
                                : undefined
                            }
                            disabled={isCustomSwatch && !selectedEmoji}
                            key={swatch}
                            onClick={() => handleColorSelect(swatch)}
                            style={{
                              background: isCustomSwatch
                                ? isSelected
                                  ? selectedColor
                                  : "conic-gradient(from 0deg, #ff4d4d, #ffe75c, #73ef75, #63c6f2, #b141ff, #ff4d4d)"
                                : swatch,
                            }}
                            type="button"
                          >
                            {isSelected ? (
                              <span
                                className={cn(
                                  "absolute rounded-full border-[3px]",
                                  isOnboardingModal ? "inset-0.5" : "inset-1",
                                )}
                                style={{
                                  borderColor: contrastColorForBackground(
                                    isCustomSwatch ? selectedColor : swatch,
                                  ),
                                }}
                              />
                            ) : null}
                          </button>
                        );
                      })}
                    </div>
                  </div>

                  <AvatarCustomColorPanel
                    colorDraft={customColorDraft}
                    hue={customHue}
                    onCommit={commitCustomColor}
                    onHueChange={setCustomHue}
                    onSaturationValueChange={(nextSaturation, nextValue) => {
                      setCustomSaturation(nextSaturation);
                      setCustomValue(nextValue);
                    }}
                    saturation={customSaturation}
                    testIdPrefix={testIdPrefix}
                    value={customValue}
                    visible={isCustomColorPickerVisible}
                  />
                </div>
              )}
            </div>
          </div>

          <AnimatePresence initial={false}>
            {shouldShowDoneButton ? (
              <Button
                asChild
                className={cn(
                  isOnboardingModal
                    ? "mx-auto mt-0 h-[2.375rem] min-w-24 rounded-full bg-[rgb(var(--buzz-onboarding-avatar-action-bg))] px-6 text-sm font-medium text-[rgb(var(--buzz-onboarding-avatar-action-fg))] hover:bg-[color:rgb(var(--buzz-onboarding-avatar-action-bg)_/_0.9)]"
                    : "mt-2 h-12 w-full rounded-xl",
                )}
              >
                <motion.button
                  animate={{ opacity: 1, scale: 1 }}
                  data-testid={`${testIdPrefix}-done`}
                  disabled={isDoneButtonDisabled}
                  exit={
                    shouldReduceMotion
                      ? { opacity: 0 }
                      : { opacity: 0, scale: 0.96 }
                  }
                  initial={
                    shouldReduceMotion
                      ? { opacity: 0 }
                      : { opacity: 0, scale: 0.98 }
                  }
                  key="done"
                  onClick={handleDoneClick}
                  transition={DONE_BUTTON_SHELL_TRANSITION}
                  type="button"
                >
                  <span className="grid place-items-center">
                    <AnimatePresence initial={false}>
                      {isDoneButtonPending && !isOnboardingModal ? (
                        <motion.span
                          animate={{ opacity: 1, y: 0 }}
                          className="col-start-1 row-start-1 inline-flex items-center justify-center gap-2"
                          exit={
                            shouldReduceMotion
                              ? { opacity: 0, y: 0 }
                              : { opacity: 0, y: -3 }
                          }
                          initial={
                            shouldReduceMotion
                              ? { opacity: 0, y: 0 }
                              : { opacity: 0, y: 3 }
                          }
                          key="pending"
                          transition={DONE_BUTTON_CONTENT_TRANSITION}
                        >
                          <Spinner
                            aria-label="Saving avatar"
                            className="h-4 w-4 border-2"
                          />
                          <span>Saving</span>
                        </motion.span>
                      ) : (
                        <motion.span
                          animate={{ opacity: 1, y: 0 }}
                          className="col-start-1 row-start-1"
                          exit={
                            shouldReduceMotion
                              ? { opacity: 0, y: 0 }
                              : { opacity: 0, y: -3 }
                          }
                          initial={
                            shouldReduceMotion
                              ? { opacity: 0, y: 0 }
                              : { opacity: 0, y: 3 }
                          }
                          key="ready"
                          transition={DONE_BUTTON_CONTENT_TRANSITION}
                        >
                          {isOnboardingModal ? "Save" : "Done"}
                        </motion.span>
                      )}
                    </AnimatePresence>
                  </span>
                </motion.button>
              </Button>
            ) : null}
          </AnimatePresence>
        </div>
      </div>
    </fieldset>
  );
}
