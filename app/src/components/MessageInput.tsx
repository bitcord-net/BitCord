import { useState, useRef, useEffect, useCallback } from "react";
import type { ChangeEvent, KeyboardEvent } from "react";
import { Send, X, CornerUpLeft, Hash, AtSign } from "lucide-react";
import type { MemberInfo, ChannelInfo } from "../lib/rpc-types";

interface ReplyTarget {
  id: string;
  author_id: string;
  body: string;
}

interface MessageInputProps {
  channelName: string;
  replyToMessage?: ReplyTarget | null;
  replyToAuthorName?: string;
  onClearReply: () => void;
  onSend: (body: string, replyToId?: string) => Promise<void>;
  members: MemberInfo[];
  channels: ChannelInfo[];
  disabled?: boolean;
}

// Slash commands palette
const SLASH_COMMANDS = [
  { name: "shrug", description: 'Append ¯\\_(ツ)_/¯', transform: (t: string) => t.replace(/^\/shrug\s*/, "") + " ¯\\_(ツ)_/¯" },
  { name: "me", description: "Describe an action", transform: (t: string) => `_${t.replace(/^\/me\s*/, "")}_` },
  { name: "spoiler", description: "Hide text as spoiler", transform: (t: string) => `||${t.replace(/^\/spoiler\s*/, "")}||` },
];

// Detects autocomplete trigger at the current position
function detectTrigger(value: string, pos: number) {
  const before = value.slice(0, pos);

  // @mention
  const mentionMatch = /@(\w*)$/.exec(before);
  if (mentionMatch) {
    return { type: "mention" as const, query: mentionMatch[1], start: before.length - mentionMatch[0].length };
  }

  // #channel
  const channelMatch = /#(\w*)$/.exec(before);
  if (channelMatch) {
    return { type: "channel" as const, query: channelMatch[1], start: before.length - channelMatch[0].length };
  }

  // / command (only at start)
  const cmdMatch = /^\/(\w*)$/.exec(before.trimStart());
  if (cmdMatch && before.trimStart().startsWith("/")) {
    return { type: "command" as const, query: cmdMatch[1], start: before.lastIndexOf("/") };
  }

  return null;
}

export function MessageInput({
  channelName,
  replyToMessage,
  replyToAuthorName,
  onClearReply,
  onSend,
  members,
  channels,
  disabled = false,
}: MessageInputProps) {
  const [value, setValue] = useState("");
  const [sending, setSending] = useState(false);
  const [autocomplete, setAutocomplete] = useState<{
    type: "mention" | "channel" | "command";
    query: string;
    start: number;
    selectedIdx: number;
  } | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // Focus textarea on mount and when reply changes
  useEffect(() => {
    textareaRef.current?.focus();
  }, [replyToMessage]);

  // Auto-resize textarea
  const autoResize = () => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 200)}px`;
  };

  const handleChange = (e: ChangeEvent<HTMLTextAreaElement>) => {
    const val = e.target.value;
    setValue(val);
    autoResize();

    const pos = e.target.selectionStart ?? val.length;
    const trigger = detectTrigger(val, pos);
    if (trigger) {
      setAutocomplete({ ...trigger, selectedIdx: 0 });
    } else {
      setAutocomplete(null);
    }
  };

  // Filtered autocomplete options
  const autocompleteOptions = (() => {
    if (!autocomplete) return [];
    const q = autocomplete.query.toLowerCase();
    if (autocomplete.type === "mention") {
      return members
        .filter((m) => m.display_name.toLowerCase().includes(q))
        .slice(0, 8)
        .map((m) => ({ label: m.display_name, value: m.display_name, sub: m.user_id.slice(0, 8) }));
    }
    if (autocomplete.type === "channel") {
      return channels
        .filter((c) => c.name.toLowerCase().includes(q))
        .slice(0, 8)
        .map((c) => ({ label: c.name, value: c.name, sub: c.kind }));
    }
    if (autocomplete.type === "command") {
      return SLASH_COMMANDS
        .filter((c) => c.name.startsWith(q))
        .map((c) => ({ label: `/${c.name}`, value: c.name, sub: c.description }));
    }
    return [];
  })();

  const applyAutocomplete = useCallback(
    (option: { label: string; value: string }) => {
      if (!autocomplete) return;
      const prefix = autocomplete.type === "mention" ? "@" : autocomplete.type === "channel" ? "#" : "/";
      const before = value.slice(0, autocomplete.start);
      const after = value.slice(textareaRef.current?.selectionStart ?? value.length);
      const insertion = `${prefix}${option.value} `;
      const newVal = before + insertion + after;
      setValue(newVal);
      setAutocomplete(null);
      setTimeout(() => {
        const el = textareaRef.current;
        if (!el) return;
        const pos = before.length + insertion.length;
        el.setSelectionRange(pos, pos);
        el.focus();
        autoResize();
      }, 0);
    },
    [autocomplete, value]
  );

  const handleSend = useCallback(async () => {
    const trimmed = value.trim();
    if (!trimmed || sending || disabled) return;

    // Check slash command transforms
    const cmdMatch = /^\/(\w+)\s*(.*)$/.exec(trimmed);
    let finalBody = trimmed;
    if (cmdMatch) {
      const cmd = SLASH_COMMANDS.find((c) => c.name === cmdMatch[1]);
      if (cmd) {
        finalBody = cmd.transform(trimmed);
      }
    }

    setSending(true);
    setValue("");
    setAutocomplete(null);
    setTimeout(() => {
      if (textareaRef.current) {
        textareaRef.current.style.height = "auto";
      }
    }, 0);

    try {
      await onSend(finalBody, replyToMessage?.id ?? undefined);
    } finally {
      setSending(false);
      textareaRef.current?.focus();
    }
  }, [value, sending, disabled, onSend, replyToMessage]);

  const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    // Navigate autocomplete
    if (autocomplete && autocompleteOptions.length > 0) {
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setAutocomplete((a) =>
          a ? { ...a, selectedIdx: (a.selectedIdx - 1 + autocompleteOptions.length) % autocompleteOptions.length } : a
        );
        return;
      }
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setAutocomplete((a) =>
          a ? { ...a, selectedIdx: (a.selectedIdx + 1) % autocompleteOptions.length } : a
        );
        return;
      }
      if (e.key === "Tab" || e.key === "Enter") {
        e.preventDefault();
        const opt = autocompleteOptions[autocomplete.selectedIdx];
        if (opt) applyAutocomplete(opt);
        return;
      }
      if (e.key === "Escape") {
        setAutocomplete(null);
        return;
      }
    }

    // Send on Enter (no shift), newline on Shift+Enter
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void handleSend();
    }
  };

  const canSend = value.trim().length > 0 && !sending && !disabled;

  return (
    <div
      style={{
        padding: "0 1rem 1rem",
        flexShrink: 0,
        position: "relative",
      }}
    >
      {/* Autocomplete dropdown */}
      {autocomplete && autocompleteOptions.length > 0 && (
        <div
          role="listbox"
          aria-label="Suggestions"
          style={{
            position: "absolute",
            bottom: "calc(100% - 0.5rem)",
            left: "1rem",
            right: "1rem",
            background: "var(--color-bc-surface-2)",
            border: "1px solid rgba(255,255,255,0.08)",
            borderRadius: "8px",
            overflow: "hidden",
            boxShadow: "0 -4px 16px rgba(0,0,0,0.4)",
            zIndex: 50,
          }}
        >
          <div
            style={{
              padding: "4px 0.75rem",
              fontSize: "0.75rem",
              color: "var(--color-bc-muted)",
              borderBottom: "1px solid rgba(255,255,255,0.06)",
              display: "flex",
              alignItems: "center",
              gap: "4px",
            }}
          >
            {autocomplete.type === "mention" && <AtSign size={12} />}
            {autocomplete.type === "channel" && <Hash size={12} />}
            {autocomplete.type === "mention" && "Members matching"}
            {autocomplete.type === "channel" && "Channels matching"}
            {autocomplete.type === "command" && "Commands"}
            <span style={{ fontWeight: 600, color: "var(--color-bc-text)" }}>
              {autocomplete.query ? `"${autocomplete.query}"` : ""}
            </span>
          </div>
          {autocompleteOptions.map((opt, i) => (
            <div
              key={opt.value}
              role="option"
              aria-selected={i === autocomplete.selectedIdx}
              onClick={() => applyAutocomplete(opt)}
              style={{
                display: "flex",
                alignItems: "center",
                gap: "0.5rem",
                padding: "0.5rem 0.75rem",
                cursor: "pointer",
                background:
                  i === autocomplete.selectedIdx
                    ? "rgba(88,101,242,0.15)"
                    : "transparent",
              }}
              onMouseEnter={() =>
                setAutocomplete((a) => (a ? { ...a, selectedIdx: i } : a))
              }
            >
              <span style={{ fontWeight: 600, fontSize: "0.9rem", color: "var(--color-bc-text)" }}>
                {opt.label}
              </span>
              <span style={{ fontSize: "0.8rem", color: "var(--color-bc-muted)" }}>
                {opt.sub}
              </span>
            </div>
          ))}
        </div>
      )}

      {/* Reply bar */}
      {replyToMessage && (
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: "0.5rem",
            padding: "0.375rem 0.625rem",
            background: "var(--color-bc-surface-2)",
            borderRadius: "6px 6px 0 0",
            borderBottom: "1px solid rgba(255,255,255,0.04)",
            fontSize: "0.8125rem",
            color: "var(--color-bc-muted)",
          }}
        >
          <CornerUpLeft size={14} style={{ flexShrink: 0 }} />
          <span style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            Replying to{" "}
            <strong style={{ color: "var(--color-bc-text)" }}>
              {replyToAuthorName ?? replyToMessage.author_id.slice(0, 8)}
            </strong>
            {" — "}
            {replyToMessage.body.slice(0, 60)}
            {replyToMessage.body.length > 60 ? "…" : ""}
          </span>
          <button
            onClick={onClearReply}
            aria-label="Cancel reply"
            style={{
              border: "none",
              background: "transparent",
              color: "var(--color-bc-muted)",
              cursor: "pointer",
              padding: "2px",
              display: "flex",
              borderRadius: "3px",
            }}
          >
            <X size={14} />
          </button>
        </div>
      )}

      {/* Input area */}
      <div
        style={{
          display: "flex",
          alignItems: "flex-end",
          gap: "0",
          background: "var(--color-bc-surface-2)",
          borderRadius: replyToMessage ? "0 0 8px 8px" : "8px",
          border: "1px solid rgba(255,255,255,0.06)",
          overflow: "hidden",
        }}
      >
        <textarea
          ref={textareaRef}
          value={value}
          onChange={handleChange}
          onKeyDown={handleKeyDown}
          placeholder={disabled ? "Loading…" : `Message #${channelName}`}
          disabled={disabled}
          rows={1}
          aria-label={`Message #${channelName}`}
          aria-multiline="true"
          style={{
            flex: 1,
            background: "transparent",
            border: "none",
            outline: "none",
            color: "var(--color-bc-text)",
            padding: "0.6875rem 0.875rem",
            fontSize: "0.9375rem",
            lineHeight: 1.375,
            resize: "none",
            fontFamily: "inherit",
            maxHeight: "200px",
            overflowY: "auto",
          }}
        />
        <button
          onClick={() => void handleSend()}
          disabled={!canSend}
          aria-label="Send message"
          title="Send message (Enter)"
          style={{
            border: "none",
            background: canSend ? "var(--color-bc-accent)" : "transparent",
            color: canSend ? "#fff" : "var(--color-bc-muted)",
            cursor: canSend ? "pointer" : "not-allowed",
            padding: "0.6rem 0.875rem",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            flexShrink: 0,
            alignSelf: "flex-end",
            transition: "background 0.15s, color 0.15s",
            borderRadius: "0 8px 8px 0",
          }}
        >
          <Send size={18} aria-hidden="true" />
        </button>
      </div>
    </div>
  );
}
