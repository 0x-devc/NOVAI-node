import { createRoot } from "react-dom/client";
import "@fontsource-variable/inter";
import "@fontsource-variable/space-grotesk";
import "../index.css";
import SpecimenApp from "./SpecimenApp";

createRoot(document.getElementById("root")!).render(<SpecimenApp />);
