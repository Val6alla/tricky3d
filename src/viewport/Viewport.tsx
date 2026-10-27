import { useEffect, useRef } from "react";
import { useUi } from "../ui";
import { registerViewport } from "./api";
import { createViewportEngine } from "./engine";
import "./viewport.css";

export function Viewport() {
  const hostRef = useRef<HTMLDivElement>(null);
  const cubeRef = useRef<HTMLDivElement>(null);
  const dropActive = useUi((s) => s.dropActive);

  useEffect(() => {
    const host = hostRef.current;
    const cube = cubeRef.current;
    if (!host || !cube) return;
    const engine = createViewportEngine(host, cube);
    registerViewport(engine.api);
    return () => {
      registerViewport(null);
      engine.dispose();
    };
  }, []);

  return (
    <div className="viewport" ref={hostRef}>
      <div className="viewport-cube" ref={cubeRef} />
      {dropActive && (
        <div className="viewport-drop" aria-hidden="true">
          Drop mesh files to import
        </div>
      )}
    </div>
  );
}
