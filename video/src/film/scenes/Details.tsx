import React from "react";
import { useCurrentFrame } from "remotion";
import { BEAT, C, enter, line, sceneLength } from "../theme";
import { Panel, Stage } from "../ui";

const FONT = 54;
const LINE_HEIGHT = 80;

/** Close-up: one finding's evidence, the reason it is safe to move. */
export const Details: React.FC = () => {
  const frame = useCurrentFrame();
  const length = sceneLength("details");
  const push = 1.0 + (0.08 * frame) / length;
  return (
    <Stage scale={push} origin="40% 70%">
      <div style={{ position: "absolute", left: 110, top: 250 }}>
        <Panel
          title={line("details.title")}
          width={1700}
          font={FONT}
          lineHeight={LINE_HEIGHT}
          rows={6}
        >
          <div style={{ fontWeight: 700 }}>{line("details.name")}</div>
          <div>{line("details.size")}</div>
          <div style={{ color: C.muted }}>{line("details.path")}</div>
          {(["details.evidence.when", "details.evidence.why"] as const).map(
            (id, i) => (
              <div key={id}>
                <span style={{ position: "relative", display: "inline-block" }}>
                  {line(id)}
                  <span
                    style={{
                      position: "absolute",
                      left: 0,
                      bottom: 4,
                      height: 6,
                      width: `${enter(frame, BEAT + i * BEAT, BEAT) * 100}%`,
                      backgroundColor: C.lime,
                    }}
                  />
                </span>
              </div>
            ),
          )}
        </Panel>
      </div>
    </Stage>
  );
};
