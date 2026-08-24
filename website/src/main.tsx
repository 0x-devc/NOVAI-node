import { createRoot } from "react-dom/client";
import App from "./App.tsx";
import "@fontsource-variable/inter";
import "@fontsource-variable/space-grotesk";
import "./index.css";

createRoot(document.getElementById("root")!).render(<App />);
