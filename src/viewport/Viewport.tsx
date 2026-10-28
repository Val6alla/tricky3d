import { lazy, Suspense, useEffect, useRef } from "react";
import { useLibrary } from "../library/store";
import { useUi } from "../ui";
import { registerViewport } from "./api";
import { createViewportEngine } from "./engine";
import "./viewport.css";

// The Part library's gallery loads when the library first opens.
const LibraryBrowser = lazy(() => import("../library/LibraryBrowser").then((m) => ({ default: m.LibraryBrowser })));

export function Viewport() {
  const hostRef = useRef<HTMLDivElement>(null);
  const cubeRef = useRef<HTMLDivElement>(null);
  const dropActive = useUi((s) => s.dropActive);
  const libraryOpen = useLibrary((s) => s.session !== null);

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
      {libraryOpen && (
        <Suspense fallback={null}>
          <LibraryBrowser />
        </Suspense>
      )}
      {dropActive && (
        <div className="viewport-drop" aria-hidden="true">
          Drop mesh files to import
        </div>
      )}
    </div>
  );
}
