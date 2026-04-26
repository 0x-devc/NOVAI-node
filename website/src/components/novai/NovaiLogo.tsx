import novaiSymbol from "@/assets/novai-symbol.png";

interface NovaiLogoProps {
  size?: number;
}

export default function NovaiLogo({ size = 32 }: NovaiLogoProps) {
  return (
    <img
      src={novaiSymbol}
      alt="NOVAInetwork"
      width={size}
      height={size}
      className="object-contain"
    />
  );
}
