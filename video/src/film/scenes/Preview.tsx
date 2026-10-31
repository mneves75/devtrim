import React from "react";
import { useCurrentFrame } from "remotion";
import { BEAT, PANEL, drift, enter, sceneLength } from "../theme";
import { Stage } from "../ui";
import { PreviewPanel } from "./PreviewPanel";

/** Wide: the preview draws, then each finding lands on a beat. */
export const Preview: React.FC = () => {
  const frame = useCurrentFrame();
  const length = sceneLength("preview");
  return (
    <Stage scale={drift(frame, length, 0.025)} origin="50% 40%">
      <div style={{ position: "absolute", left: PANEL.left, top: 380 }}>
        <PreviewPanel
          frame={frame}
          draw={enter(frame, 0, BEAT)}
          arrivals={[BEAT, 2 * BEAT, 3 * BEAT, 4 * BEAT]}
        />
      </div>
    </Stage>
  );
};
