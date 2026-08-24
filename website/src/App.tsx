import { Toaster } from "@/components/ui/toaster";
import { Toaster as Sonner } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import ParticleField from "@/components/novai/ParticleField";
import Navbar from "@/components/novai/Navbar";
import ScrollProgress from "@/components/novai/ScrollProgress";
import SinglePage from "./pages/SinglePage";

const queryClient = new QueryClient();

const App = () => (
  <QueryClientProvider client={queryClient}>
    <TooltipProvider>
      <Toaster />
      <Sonner />
      <ScrollProgress />
      <ParticleField />
      <Navbar />
      <SinglePage />
    </TooltipProvider>
  </QueryClientProvider>
);

export default App;
