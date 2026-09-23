import { Sidebar } from "../features/layout/Sidebar";
import { StoryView } from "../features/ledger/StoryView";
import { useNarrationEvents } from "./useNarrationEvents";

function App() {
  useNarrationEvents();

  return (
    <div className="flex h-screen w-screen bg-bg text-text">
      <Sidebar />
      <main className="flex flex-1 flex-col overflow-hidden">
        <StoryView />
      </main>
    </div>
  );
}

export default App;
