import React from "react";
import { AbsoluteFill, interpolate } from "remotion";
import { C, EASE_IN, MONO, PANEL, RULE, SANS, enter, line } from "./theme";

/** A scene's background and its slow push. */
export const Stage: React.FC<{
  scale?: number;
  origin?: string;
  children: React.ReactNode;
}> = ({ scale = 1, origin = "50% 50%", children }) => (
  <AbsoluteFill style={{ backgroundColor: C.bg, overflow: "hidden" }}>
    <AbsoluteFill
      style={{ transform: `scale(${scale})`, transformOrigin: origin }}
    >
      {children}
    </AbsoluteFill>
  </AbsoluteFill>
);

/** The site's favicon.svg, path for path, its near-white stroke drawn in ink; only the lime bar's scale moves. */
export const Logo: React.FC<{ size: number; bar?: number }> = ({
  size,
  bar = 1,
}) => (
  <svg
    width={size}
    height={size}
    viewBox="0 0 64 64"
    role="img"
    aria-label="devtrim"
  >
    <rect width="64" height="64" rx="12" fill={C.bg} />
    <path
      d="M17 14v36h12c12 0 18-7 18-18s-6-18-18-18H17Z"
      fill="none"
      stroke={C.ink}
      strokeWidth="7"
      strokeLinejoin="round"
    />
    <rect
      x="52"
      y="14"
      width="5"
      height="36"
      rx="2.5"
      fill={C.lime}
      style={{ transform: `scaleY(${bar})`, transformOrigin: "54.5px 50px" }}
    />
  </svg>
);

/** A Ratatui block: a ruled box whose title sits in its top border. */
export const Panel: React.FC<{
  title: React.ReactNode;
  width: number;
  font: number;
  lineHeight: number;
  rows: number;
  draw?: number;
  children: React.ReactNode;
}> = ({ title, width, font, lineHeight, rows, draw = 1, children }) => (
  <div
    style={{
      position: "relative",
      width,
      height: rows * lineHeight + lineHeight,
      fontFamily: MONO,
      fontSize: font,
      lineHeight: `${lineHeight}px`,
      color: C.ink,
      whiteSpace: "pre",
    }}
  >
    <div
      style={{
        position: "absolute",
        inset: `${lineHeight / 2}px 0 0 0`,
        border: `2px solid ${RULE}`,
        clipPath: `inset(0 ${(1 - draw) * 100}% 0 0)`,
      }}
    />
    {title === null ? null : (
      <div
        style={{
          position: "absolute",
          left: font,
          top: 0,
          padding: `0 ${font / 2}px`,
          backgroundColor: C.bg,
        }}
      >
        {title}
      </div>
    )}
    <div
      style={{
        position: "absolute",
        left: font * 0.75,
        top: lineHeight * 1.15,
        right: font * 0.75,
        bottom: 2,
        // A row leaving the plan disappears at the border instead of crossing it.
        overflow: "hidden",
      }}
    >
      {children}
    </div>
  </div>
);

const ROW_TOKENS =
  /(danger-5|TRASH|stale [^…]+?(?= {2})|…\S+|\d+(?:\.\d+)? [GM]B)/;

/** One finding row, coloured the way the TUI colours it; a row left out reads muted. */
export const RowText: React.FC<{ text: string; quiet?: boolean }> = ({
  text,
  quiet = false,
}) =>
  quiet ? (
    <span style={{ color: C.muted }}>{text}</span>
  ) : (
    <>
      {text.split(ROW_TOKENS).map((part, i) => {
        if (part === "danger-5")
          return (
            <span key={i} style={{ color: C.amber }}>
              {part}
            </span>
          );
        if (part === "TRASH")
          return (
            <span key={i} style={{ color: C.lime }}>
              {part}
            </span>
          );
        if (part.startsWith("stale "))
          return (
            <span key={i} style={{ fontWeight: 700 }}>
              {part}
            </span>
          );
        if (part.startsWith("…"))
          return (
            <span key={i} style={{ color: C.muted }}>
              {part}
            </span>
          );
        if (/^\d/.test(part) && /[GM]B$/.test(part))
          return (
            <span key={i} style={{ fontWeight: 700 }}>
              {part}
            </span>
          );
        return <span key={i}>{part}</span>;
      })}
    </>
  );

/** The box title of the preview (its selection, size and danger), the danger picked out. */
export const PreviewTitle: React.FC<{ text: string }> = ({ text }) => (
  <span>
    {text.split(/(danger-5)/).map((part, i) => (
      <span
        key={i}
        style={
          part === "danger-5" ? { color: C.amber, fontWeight: 700 } : undefined
        }
      >
        {part}
      </span>
    ))}
  </span>
);

/** `[x]` with a mark that can be taken away; the TUI shows `[ ]` for a finding left out. */
export const Checkbox: React.FC<{ mark: number }> = ({ mark }) => (
  <span>
    {line("row.open")}
    <span style={{ opacity: mark, color: C.lime }}>{line("row.mark")}</span>
    {line("row.close")}
  </span>
);

/** A key being pressed: it pops in on its frame and settles. */
export const Keycap: React.FC<{
  label: string;
  frame: number;
  at: number;
  size?: number;
}> = ({ label, frame, at, size = 44 }) => {
  const shown = enter(frame, at - 4, 10);
  const press = interpolate(frame, [at, at + 4, at + 12], [1, 0.92, 1], {
    easing: EASE_IN,
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  const lit = interpolate(frame, [at, at + 20], [1, 0], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  return (
    <div
      style={{
        display: "inline-block",
        minWidth: size * 1.6,
        padding: `${size * 0.25}px ${size * 0.5}px`,
        border: `3px solid ${frame >= at && lit > 0.01 ? C.lime : RULE}`,
        borderRadius: 10,
        backgroundColor: C.bg,
        color: frame >= at && lit > 0.5 ? C.lime : C.ink,
        fontFamily: MONO,
        fontSize: size,
        fontWeight: 700,
        textAlign: "center",
        opacity: shown,
        transform: `translateY(${(1 - shown) * 16}px) scale(${press})`,
      }}
    >
      {label}
    </div>
  );
};

/** A line of type whose words arrive one after another at reading speed. */
export const Words: React.FC<{
  text: string;
  frame: number;
  start: number;
  stagger?: number;
  style?: React.CSSProperties;
}> = ({ text, frame, start, stagger = 5, style }) => (
  <div style={{ fontFamily: SANS, ...style }}>
    {text.split(" ").map((word, i) => {
      const t = enter(frame, start + i * stagger, 20);
      return (
        <span
          key={i}
          style={{
            display: "inline-block",
            opacity: t,
            transform: `translateY(${(1 - t) * 0.35}em)`,
            marginRight: "0.25em",
          }}
        >
          {word}
        </span>
      );
    })}
  </div>
);

export { PANEL };
