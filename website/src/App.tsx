import { Toaster } from "@/components/ui/toaster";
import { Toaster as Sonner } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter, Routes, Route } from "react-router-dom";
import ParticleField from "@/components/novai/ParticleField";
import Navbar from "@/components/novai/Navbar";
import HomePage from "./pages/HomePage";
import VisionPage from "./pages/VisionPage";
import SocialsPage from "./pages/SocialsPage";
import DocumentsPage from "./pages/DocumentsPage";
import TestnetPage from "./pages/TestnetPage";
import NotFound from "./pages/NotFound";

const queryClient = new QueryClient();

const App = () => (
  <QueryClientProvider client={queryClient}>
    <TooltipProvider>
      <Toaster />
      <Sonner />
      <BrowserRouter>
        <ParticleField />
        <Navbar />
        <Routes>
          <Route path="/" element={<HomePage />} />
          <Route path="/vision" element={<VisionPage />} />
          <Route path="/socials" element={<SocialsPage />} />
          <Route path="/testnet" element={<TestnetPage />} />
          <Route path="/documents" element={<DocumentsPage />} />
          <Route path="*" element={<NotFound />} />
        </Routes>
      </BrowserRouter>
    </TooltipProvider>
  </QueryClientProvider>
);

export default App;
