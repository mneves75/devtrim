import React from "react";
import { useCurrentFrame } from "remotion";
import { C, PANEL, drift, keyCues, line, move, sceneLength } from "../theme";
import { Keycap, Panel, Stage } from "../ui";
import { PreviewPanel } from "./PreviewPanel";

const keys = keyCues("select");
const at = (key: string, nth = 0) => keys.filter((k) => k.key === key)[nth].at;

/** Wide: j, j, Space — the cursor moves twice and the third finding is left out. */
export const Select: React.FC = () => {
  const frame = useCurrentFrame();
  const length = sceneLength("select");
  const j1 = at("j", 0);
  const j2 = at("j", 1);
  const space = at("space");
  const cursor = move(frame, j1, 0, 1, 8) + move(frame, j2, 0, 1, 8);
  const left = move(frame, space, 1, 0, 10);
  const label = (key: string) =>
    key === "j" ? line("key.j") : line("key.space");
  return (
    <Stage scale={drift(frame, length, 0.02)} origin="50% 30%">
      <div style={{ position: "absolute", left: PANEL.left, top: 110 }}>
        <PreviewPanel
          frame={frame}
          cursor={cursor}
          thirdSelected={left}
          narrowed={1 - left}
        />
      </div>
      <div style={{ position: "absolute", left: PANEL.left, top: 470 }}>
        <Panel
          title={null}
          width={PANEL.width}
          font={PANEL.font}
          lineHeight={PANEL.lineHeight}
          rows={2}
        >
          <div>{line("footer.keys")}</div>
          <div style={{ color: C.muted }}>{line("footer.status")}</div>
        </Panel>
      </div>
      <div
        style={{
          position: "absolute",
          left: 0,
          right: 0,
          top: 720,
          display: "flex",
          justifyContent: "center",
          gap: 40,
        }}
      >
        {keys.map((k, i) => (
          <Keycap
            key={i}
            label={label(k.key)}
            frame={frame}
            at={k.at}
            size={64}
          />
        ))}
      </div>
    </Stage>
  );
};
