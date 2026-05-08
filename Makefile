.PHONY: test-e2e test-server test-android venv-setup

# ── End-to-end latency tests (T14) ────────────────────────────────────────────
test-e2e: venv-setup
	tests/.venv/bin/pytest tests/integration -v -s --tb=short

# ── Server unit + integration tests ───────────────────────────────────────────
test-server:
	cd server && cargo test --workspace

# ── Android instrumented tests (connected device required) ────────────────────
test-android:
	cd android && ./gradlew :app:connectedDebugAndroidTest :opus-jni:connectedDebugAndroidTest

# ── Set up the Python venv ────────────────────────────────────────────────────
venv-setup:
	@bash tests/.venv-setup.sh
