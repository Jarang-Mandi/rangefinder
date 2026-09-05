import { render } from "preact";
import "../shared/tokens.css";
import "./panel.css";
import { App } from "./App";

const root = document.getElementById("root");
if (!root) throw new Error("elemen #root tidak ada di index.html");
render(<App />, root);
