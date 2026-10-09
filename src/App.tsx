import { useState } from "react";
import Settings from "./Settings";
import Caffeine from "./Caffeine";

export default function App() {
  const [showSettings, setShowSettings] = useState(false);
  return (
    <div className="app">
      <aside className="sidebar">
        <h1>Murim</h1>
        <button onClick={() => setShowSettings(!showSettings)}>Settings</button>
        <div className="label">Repos</div>
        <ul className="repos" />
        <footer className="sidebar-footer"><Caffeine /></footer>
      </aside>
      <main className="main">{showSettings ? <Settings /> : "No tasks yet"}</main>
    </div>
  );
}
