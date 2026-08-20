import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";

class AppErrorBoundary extends React.Component<React.PropsWithChildren, { error?: string }> {
  state: { error?: string } = {};

  static getDerivedStateFromError(error: unknown) {
    return { error: error instanceof Error ? error.message : String(error) };
  }

  componentDidCatch(error: unknown) {
    console.error("Reactor UI error", error);
  }

  render() {
    if (this.state.error) {
      return <div className="fatal-error card"><h1>Reactor 界面遇到异常</h1><p>{this.state.error}</p><button className="primary-button" onClick={() => window.location.reload()}>重新加载界面</button></div>;
    }
    return this.props.children;
  }
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <AppErrorBoundary><App /></AppErrorBoundary>
  </React.StrictMode>,
);
