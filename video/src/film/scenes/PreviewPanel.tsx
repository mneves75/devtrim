import React from "react";
import { BEAT, C, PANEL, enter, line } from "../theme";
import { Checkbox, Panel, PreviewTitle, RowText } from "../ui";

/**
 * Phone players shrink the frame about five times, so the panel never shows a
 * whole TUI line: either its start (checkbox, number, danger, size) or its end
 * (size, action, name). Each is still a substring of the line on the site.
 */
const START_ROWS = [
  "row.1.short",
  "row.2.short",
  "row.3.short",
  "row.4.short",
] as const;
const END_ROWS = ["row.1.end", "row.2.end", "row.3.end", "row.4.end"] as const;

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
  /** Which part of each line: "start" keeps the checkbox and cursor, "end" the name. */
  part: "start" | "end";
  font: number;
  lineHeight: number;
  titleFont?: number;
}> = ({
  frame,
  draw = 1,
  arrivals,
  cursor = 0,
  thirdSelected = 1,
  narrowed = 0,
  offsets,
  part,
  font,
  lineHeight,
  titleFont = 52,
}) => {
  const ids = part === "start" ? START_ROWS : END_ROWS;
  const title = (
    <span
      style={{
        position: "relative",
        display: "inline-block",
        fontSize: titleFont,
      }}
    >
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
      font={font}
      lineHeight={lineHeight}
      rows={5}
      draw={draw}
    >
      <div style={{ position: "relative" }}>
        {part === "start" ? (
          <div
            style={{
              position: "absolute",
              left: 0,
              top: cursor * lineHeight,
              color: C.lime,
              fontWeight: 700,
              opacity: 0.7 + 0.3 * Math.cos((frame / BEAT) * Math.PI),
            }}
          >
            {line("row.cursor")}
          </div>
        ) : null}
        {ids.map((id, i) => {
          const t = arrivals ? enter(frame, arrivals[i], 16) : 1;
          const left = i === 2 ? thirdSelected : 1;
          return (
            <div
              key={id}
              style={{
                height: lineHeight,
                paddingLeft: part === "start" ? font * 1.4 : 0,
                opacity: t,
                transform: `translate(${offsets?.[i] ?? 0}px, ${(1 - t) * 28}px)`,
              }}
            >
              {part === "start" ? (
                <>
                  <Checkbox mark={left} />
                  {/* The TUI puts three cells between the checkbox and the row number. */}
                  <span style={{ display: "inline-block", width: "3ch" }} />
                </>
              ) : null}
              <RowText text={line(id)} quiet={left < 0.5} />
            </div>
          );
        })}
      </div>
    </Panel>
  );
};
