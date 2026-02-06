const appDiv = document.getElementById("app");
if (appDiv) {
  appDiv.textContent = "Oxcer ready (local Tauri binary)";
}

const testButton = document.getElementById("test-fs");
if (testButton) {
  testButton.addEventListener("click", () => {
    console.log("Test FS clicked - wiring to Tauri fs API comes next.");
    alert("Test FS clicked (placeholder)");
  });
}

