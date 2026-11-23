import { label } from "cjs-lib";
import data from "./data.json";
import "./styles.css";
import { Counter } from "./Counter.tsx";

document.getElementById("root")!.textContent = `${label} ${data.name} ${__APP__} ${typeof Counter}`;
