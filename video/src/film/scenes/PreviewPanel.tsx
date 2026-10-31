import React from "react";
import { BEAT, C, PANEL, enter, line } from "../theme";
import { Checkbox, Panel, PreviewTitle, RowText } from "../ui";

const ROWS = ["row.1", "row.2", "row.3", "row.4"] as const;

/**
 * The TUI's preview block, shared by every wide shot so the cuts hand off on
 * the same geometry. Each prop is a frame-driven state, not an animation.
 */
export const PreviewPanel: React.FC<{
  frame: number;
  /** 0..1: how much of the box outline is drawn. */
  draw?: number;
  /** Frame each row arrives on; undefined = already there. */
  arrivals?: number[];
  /** Row index the cursor sits on, as a fractional position while it moves. */
  cursor?: number;
  /** 1 = row 3 still selected, 0 = left out. */
  thirdSelected?: number;
  /** 0 = 4/4 title, 1 = 3/4 title. */
  narrowed?: number;
  /** Horizontal offset per row (px), for rows leaving the frame. */
  offsets?: number[];
}> = ({
  frame,
  draw = 1,
  arrivals,
  cursor = 0,
  thirdSelected = 1,
  narrowed = 0,
  offsets,
}) => {
  const title = (
    <span style={{ position: "relative", display: "inline-block" }}>
      <span style={{ opacity: 1 - narrowed }}>
        <PreviewTitle text={line("preview.title")} />
      </span>
      <span
        style={{ position: "absolute", left: 0, top: 0, opacity: narrowed }}
      >
        <PreviewTitle text={line("preview.title.narrowed")} />
      </span>
    </span>
  );
  return (
    <Panel
      title={title}
      width={PANEL.width}
      font={PANEL.font}
      lineHeight={PANEL.lineHeight}
      rows={6}
      draw={draw}
    >
      <div style={{ position: "relative" }}>
        <div
          style={{
            position: "absolute",
            left: 0,
            top: cursor * PANEL.lineHeight,
            color: C.lime,
            fontWeight: 700,
            opacity: 0.7 + 0.3 * Math.cos((frame / BEAT) * Math.PI),
          }}
        >
          {line("row.cursor")}
        </div>
        {ROWS.map((id, i) => {
          const t = arrivals ? enter(frame, arrivals[i], 16) : 1;
          const left = i === 2 ? thirdSelected : 1;
          return (
            <div
              key={id}
              style={{
                height: PANEL.lineHeight,
                paddingLeft: PANEL.font * 1.4,
                opacity: t * (0.45 + 0.55 * left),
                transform: `translate(${offsets?.[i] ?? 0}px, ${(1 - t) * 28}px)`,
              }}
            >
              <Checkbox mark={left} />
              {/* The TUI puts three cells between the checkbox and the row number. */}
              <span style={{ display: "inline-block", width: "3ch" }} />
              <RowText text={line(id)} />
            </div>
          );
        })}
      </div>
    </Panel>
  );
};
