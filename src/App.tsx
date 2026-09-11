import { Sidebar } from "./components/sidebar/Sidebar";
import { StoryView } from "./components/story/StoryView";
import { useNarrationEvents } from "./lib/useNarrationEvents";

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
