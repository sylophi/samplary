import "./styles.css";

import { createRoot } from "react-dom/client";

import { App } from "@/app";
import { api } from "@/lib/api";

// The calls, for the inspector and scripted demos.
if (import.meta.env.DEV) Object.assign(window, { api });

createRoot(document.getElementById("root")!).render(<App />);
