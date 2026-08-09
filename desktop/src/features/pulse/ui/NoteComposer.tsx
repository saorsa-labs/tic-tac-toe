import * as React from "react";

import { EditorContent } from "@tiptap/react";
import { useChannelLinks } from "@/features/messages/lib/useChannelLinks";
import type { ChannelSuggestion } from "@/features/messages/lib/useChannelLinks";
import { useMentions } from "@/features/messages/lib/useMentions";
import {
  hasMentionClipboardHtml,
  normalizeMentionClipboardHtml,
} from "@/features/messages/lib/normalizeMentionClipboard";
import {
  type LinkSelectionInfo,
  useRichTextEditor,
} from "@/features/messages/lib/useRichTextEditor";
import { useLinkEditor } from "@/features/messages/lib/useLinkEditor";
import type { MentionSuggestion } from "@/features/messages/ui/MentionAutocomplete";
import { MessageComposerToolbar } from "@/features/messages/ui/MessageComposerToolbar";
import { Button } from "@/shared/ui/button";
import { cn } from "@/shared/lib/cn";
import type { NoteComposerProps } from "./NoteComposer.types";
import { NoteComposerAutocompletes } from "./NoteComposerAutocompletes";
import { NoteComposerCompactLayout } from "./NoteComposerCompactLayout";
import { useCompactComposerInteractions } from "./useCompactComposerInteractions";

export function NoteComposer({
  channelId = null,
  members,
  className,
  placeholder,
  disabled,
  header,
  isSending,
  onCancel,
  onSubmit,
  compact = false,
  autocompleteBelow = false,
  profiles,
}: NoteComposerProps) {
  const [content, setContent] = React.useState("");
  const contentRef = React.useRef(content);
  contentRef.current = content;

  const [isCompactExpanded, setIsCompactExpanded] = React.useState(!compact);
  const [isEmojiPickerOpen, setIsEmojiPickerOpen] = React.useState(false);
  const [isFormattingOpen, setIsFormattingOpen] = React.useState(false);

  const handleFormattingToggle = React.useCallback((pressed: boolean) => {
    if (pressed) setIsEmojiPickerOpen(false);
    setIsFormattingOpen(pressed);
  }, []);
  const expandCompactComposer = React.useCallback(() => {
    if (compact) setIsCompactExpanded(true);
  }, [compact]);

  const mentions = useMentions(channelId, members, profiles);
  const channelLinks = useChannelLinks();
  const { handleToolbarMouseDown, shouldIgnoreBlur } =
    useCompactComposerInteractions({
      compact,
      onExpand: expandCompactComposer,
    });

  const disabledRef = React.useRef(disabled);
  const isSendingRef = React.useRef(isSending);
  const onSubmitRef = React.useRef(onSubmit);
  disabledRef.current = disabled;
  isSendingRef.current = isSending;
  onSubmitRef.current = onSubmit;

  const isAutocompleteOpenRef = React.useRef(false);
  isAutocompleteOpenRef.current =
    mentions.isMentionOpen || channelLinks.isChannelOpen;

  const submitMessageRef = React.useRef<() => void>(() => {});

  // Set after `useLinkEditor` exists; the editor's link-click handler
  // delegates through this ref to break the hook ordering cycle.
  const onEditLinkRef = React.useRef<
    ((info: LinkSelectionInfo) => void) | null
  >(null);
  const onLinkSelectionChangeRef = React.useRef<
    ((info: LinkSelectionInfo | null) => void) | null
  >(null);
  const onLinkShortcutRef = React.useRef<(() => boolean) | null>(null);

  const richText = useRichTextEditor({
    placeholder,
    editable: !disabled,
    mentionNames: mentions.knownNames,
    channelNames: channelLinks.knownChannelNames,
    onSubmit: () => submitMessageRef.current(),
    isAutocompleteOpen: isAutocompleteOpenRef,
    onEditLink: (info) => onEditLinkRef.current?.(info),
    onLinkSelectionChange: (info) => onLinkSelectionChangeRef.current?.(info),
    onLinkShortcut: () => onLinkShortcutRef.current?.() ?? false,
    onUpdate: ({ cursor, text }) => {
      const markdown = richText.getMarkdown();
      setContent(markdown);
      contentRef.current = markdown;

      mentions.updateMentionQuery(text, cursor);
      channelLinks.updateChannelQuery(text, cursor);
    },
  });

  const linkEditor = useLinkEditor(richText);
  onEditLinkRef.current = linkEditor.openFromClick;
  onLinkSelectionChangeRef.current = linkEditor.showFromCursor;
  onLinkShortcutRef.current = linkEditor.openFromShortcut;

  // ── Mention / channel autocomplete insertion ────────────────────────
  // Native ProseMirror transactions — no markdown round-trip.
  const applyMentionInsert = React.useCallback(
    (suggestion: MentionSuggestion) => {
      const { cursor } = richText.getPlainTextAndCursor();
      const { replaceFromOffset, replaceToOffset, insertText } =
        mentions.insertMention(suggestion, cursor);
      richText.replacePlainTextRange(
        replaceFromOffset,
        replaceToOffset,
        insertText,
      );
    },
    [
      mentions.insertMention,
      richText.getPlainTextAndCursor,
      richText.replacePlainTextRange,
    ],
  );

  const applyChannelInsert = React.useCallback(
    (suggestion: ChannelSuggestion) => {
      const { cursor } = richText.getPlainTextAndCursor();
      const { replaceFromOffset, replaceToOffset, insertText } =
        channelLinks.insertChannel(suggestion, cursor);
      richText.replacePlainTextRange(
        replaceFromOffset,
        replaceToOffset,
        insertText,
      );
    },
    [
      channelLinks.insertChannel,
      richText.getPlainTextAndCursor,
      richText.replacePlainTextRange,
    ],
  );

  const insertEmoji = React.useCallback(
    (emoji: string) => {
      if (!richText.editor) return;
      richText.editor.chain().focus().insertContent(emoji).run();
      setIsEmojiPickerOpen(false);
      mentions.clearMentions();
    },
    [richText.editor, mentions.clearMentions],
  );

  // ── @ mention picker (toolbar button) ───────────────────────────────
  const openMentionPicker = React.useCallback(() => {
    if (!richText.editor) return;
    const { text, cursor } = richText.getPlainTextAndCursor();

    const beforeCursor = text.slice(0, cursor);
    if (/(?:^|[\s])@[^\s]*$/.test(beforeCursor)) {
      mentions.updateMentionQuery(text, cursor);
      richText.focus();
      return;
    }

    const previousChar = text.slice(0, cursor).slice(-1);
    const prefix =
      cursor > 0 && previousChar && !/\s/.test(previousChar) ? " @" : "@";
    richText.editor.chain().focus().insertContent(prefix).run();
    setIsEmojiPickerOpen(false);

    const { text: updatedText, cursor: updatedCursor } =
      richText.getPlainTextAndCursor();
    mentions.updateMentionQuery(updatedText, updatedCursor);
  }, [
    richText.editor,
    richText.getPlainTextAndCursor,
    richText.focus,
    mentions.updateMentionQuery,
  ]);

  // ── Submit ──────────────────────────────────────────────────────────
  const submitMessage = React.useCallback(() => {
    const trimmed = contentRef.current.trim();

    if (!trimmed || disabledRef.current || isSendingRef.current) {
      return;
    }

    const pubkeys = mentions.extractMentionPubkeys(trimmed);

    // Save draft state so we can restore on failure.
    const savedContent = contentRef.current;

    setContent("");
    contentRef.current = "";
    richText.clearContent();
    mentions.clearMentions();
    channelLinks.clearChannels();
    setIsEmojiPickerOpen(false);

    const result = onSubmitRef.current(trimmed, pubkeys);
    const collapseCompactComposer = () => {
      if (compact) setIsCompactExpanded(false);
    };

    // If onSubmit returns a promise, restore draft on failure.
    if (result && typeof result.then === "function") {
      result.then(collapseCompactComposer).catch(() => {
        setContent(savedContent);
        contentRef.current = savedContent;
        richText.setContent(savedContent);
        if (compact) setIsCompactExpanded(true);
      });
    } else {
      collapseCompactComposer();
    }
  }, [
    compact,
    mentions.extractMentionPubkeys,
    mentions.clearMentions,
    channelLinks.clearChannels,
    richText.clearContent,
    richText.setContent,
  ]);
  submitMessageRef.current = submitMessage;

  const handleSubmit = React.useCallback(
    (event: React.FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      submitMessage();
    },
    [submitMessage],
  );

  // ── Keyboard handling ───────────────────────────────────────────────
  const handleEditorKeyDown = React.useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      const channelResult = channelLinks.handleChannelKeyDown(event);
      if (channelResult.handled) {
        if (channelResult.suggestion) {
          applyChannelInsert(channelResult.suggestion);
        }
        return;
      }

      const { handled, suggestion } = mentions.handleMentionKeyDown(event);
      if (handled) {
        if (suggestion) {
          applyMentionInsert(suggestion);
        }
        return;
      }

      if (event.key === "Tab" && !event.shiftKey && linkEditor.isCardOpen) {
        event.preventDefault();
        if (!linkEditor.focusCardFirstControl()) {
          requestAnimationFrame(linkEditor.focusCardFirstControl);
        }
        return;
      }
    },
    [
      channelLinks.handleChannelKeyDown,
      applyChannelInsert,
      mentions.handleMentionKeyDown,
      applyMentionInsert,
      linkEditor.isCardOpen,
      linkEditor.focusCardFirstControl,
    ],
  );

  // ── Clipboard normalization ────────────────────────────────────────

  React.useEffect(() => {
    if (!richText.editor) return;

    richText.editor.setOptions({
      editorProps: {
        ...richText.editor.options.editorProps,
        handlePaste: (_view, event) => {
          const html = event.clipboardData?.getData("text/html");
          if (html && hasMentionClipboardHtml(html)) {
            const cleanHtml = normalizeMentionClipboardHtml(html);
            event.preventDefault();
            _view.pasteHTML(cleanHtml);
            return true;
          }

          return false;
        },
      },
    });
  }, [richText.editor]);

  const sendDisabled = React.useMemo(
    () => disabled || content.trim().length === 0,
    [disabled, content],
  );
  const hasComposerContent = content.trim().length > 0;
  const isExpanded =
    !compact ||
    isCompactExpanded ||
    hasComposerContent ||
    isEmojiPickerOpen ||
    isFormattingOpen ||
    mentions.isMentionOpen ||
    channelLinks.isChannelOpen;
  const isCompactLayout = compact && !isExpanded;
  const handleFormBlur = React.useCallback(
    (event: React.FocusEvent<HTMLFormElement>) => {
      if (!compact) return;

      const nextTarget = event.relatedTarget;
      if (
        nextTarget instanceof Node &&
        event.currentTarget.contains(nextTarget)
      ) {
        return;
      }
      if (shouldIgnoreBlur()) {
        return;
      }

      const hasDraft =
        contentRef.current.trim().length > 0 ||
        isEmojiPickerOpen ||
        isFormattingOpen;

      if (!hasDraft) setIsCompactExpanded(false);
    },
    [compact, isEmojiPickerOpen, isFormattingOpen, shouldIgnoreBlur],
  );
  const wasCompactExpandedRef = React.useRef(isCompactExpanded);
  React.useEffect(() => {
    const wasExpanded = wasCompactExpandedRef.current;
    wasCompactExpandedRef.current = isCompactExpanded;

    if (!compact || !isCompactExpanded || wasExpanded) return;

    const frame = window.requestAnimationFrame(() => {
      richText.focus();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [compact, isCompactExpanded, richText.focus]);
  const autocompletePosition = autocompleteBelow ? "below" : "above";
  return (
    <>
      <form
        className={cn(
          "relative rounded-2xl border border-input bg-card px-3 py-2 sm:px-4",
          className,
        )}
        onBlurCapture={handleFormBlur}
        onFocusCapture={expandCompactComposer}
        onSubmit={handleSubmit}
      >
        {isCompactLayout ? (
          <NoteComposerCompactLayout
            editor={richText.editor}
            header={header}
            isSending={isSending}
            onEditorKeyDown={handleEditorKeyDown}
            sendDisabled={sendDisabled}
          />
        ) : (
          <>
            {header ? (
              <div
                className={cn("mb-2", compact && "flex min-h-10 items-center")}
              >
                {header}
              </div>
            ) : null}
            <NoteComposerAutocompletes
              channelSelectedIndex={channelLinks.channelSelectedIndex}
              channelSuggestions={
                channelLinks.isChannelOpen
                  ? channelLinks.channelSuggestions
                  : []
              }
              mentionSelectedIndex={mentions.mentionSelectedIndex}
              mentionSuggestions={
                mentions.isMentionOpen ? mentions.suggestions : []
              }
              onChannelSelect={applyChannelInsert}
              onMentionFetchMore={mentions.fetchMoreSuggestions}
              onMentionSelect={applyMentionInsert}
              position={autocompletePosition}
            />

            {/* biome-ignore lint/a11y/noStaticElementInteractions: keydown handler bridges Tiptap editor to autocomplete and submit */}
            <div
              className="rich-text-composer max-h-32 overflow-y-auto"
              onKeyDown={handleEditorKeyDown}
            >
              <EditorContent editor={richText.editor} />
            </div>

            <MessageComposerToolbar
              composerDisabled={disabled ?? false}
              editor={richText.editor}
              extraActions={
                onCancel ? (
                  <Button
                    disabled={isSending}
                    onClick={onCancel}
                    size="sm"
                    type="button"
                    variant="ghost"
                  >
                    Cancel
                  </Button>
                ) : undefined
              }
              formattingDisabled={disabled ?? false}
              isEmojiPickerOpen={isEmojiPickerOpen}
              isFormattingOpen={isFormattingOpen}
              isSending={isSending ?? false}
              onCaptureSelection={handleToolbarMouseDown}
              onEmojiPickerOpenChange={setIsEmojiPickerOpen}
              onEmojiSelect={insertEmoji}
              onFormattingToggle={handleFormattingToggle}
              onLinkButton={linkEditor.openFromToolbar}
              onOpenMentionPicker={openMentionPicker}
              sendDisabled={sendDisabled}
            />
          </>
        )}
      </form>
      {linkEditor.card}
      {linkEditor.dialog}
    </>
  );
}
