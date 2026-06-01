import { Link } from "react-router-dom";
import { shortHex } from "../lib/format";

type HashLinkProps = {
  value: string;
  /** Where the link should route. */
  kind: "block-hash" | "tx" | "address" | "entity";
  /** Show the full hex instead of shortened. */
  full?: boolean;
};

export default function HashLink({ value, kind, full = false }: HashLinkProps) {
  let href = "";
  switch (kind) {
    case "block-hash":
      // We don't know the height yet — route to a block-by-hash search;
      // BlockDetail accepts both height and hash via the heightOrHash param.
      href = `/blocks/${value}`;
      break;
    case "tx":
      href = `/tx/${value}`;
      break;
    case "address":
      href = `/account/${value}`;
      break;
    case "entity":
      href = `/entity/${value}`;
      break;
  }
  return (
    <Link
      to={href}
      className="hex hover:underline hover:text-sky-200"
      title={value}
    >
      {full ? value : shortHex(value)}
    </Link>
  );
}
