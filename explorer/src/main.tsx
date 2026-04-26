import React from "react";
import ReactDOM from "react-dom/client";
import { createBrowserRouter, RouterProvider, Navigate } from "react-router-dom";
import App from "./App";
import Blocks from "./pages/Blocks";
import BlockDetail from "./pages/BlockDetail";
import TxDetail from "./pages/TxDetail";
import Account from "./pages/Account";
import Entity from "./pages/Entity";
import Stats from "./pages/Stats";
import NotFound from "./pages/NotFound";
import "./index.css";

const router = createBrowserRouter([
  {
    path: "/",
    element: <App />,
    children: [
      { index: true, element: <Navigate to="/blocks" replace /> },
      { path: "blocks", element: <Blocks /> },
      { path: "blocks/:heightOrHash", element: <BlockDetail /> },
      { path: "tx/:txid", element: <TxDetail /> },
      { path: "account/:address", element: <Account /> },
      { path: "entity/:id", element: <Entity /> },
      { path: "stats", element: <Stats /> },
      { path: "*", element: <NotFound /> },
    ],
  },
]);

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <RouterProvider router={router} />
  </React.StrictMode>,
);
