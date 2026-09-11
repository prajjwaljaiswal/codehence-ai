import { useEffect, useState } from "react";
import { Loader2, Send, MessageSquare } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { api, errorMessage, type SlackSettings as Settings } from "@/lib/api";

interface SlackSettingsProps {
  setToast: (toast: { message: string; type: "success" | "error" } | null) => void;
}

const BLANK: Settings = {
  enabled: false,
  bot_token: "",
  channel: "dotsquares-ai",
  notify_runs: true,
};

/**
 * Slack delivery for agent questions and run outcomes.
 *
 * Saves and tests itself rather than going through the dialog's global save
 * button: the test posts with whatever is *stored*, so it would otherwise
 * check the previous token.
 */
export function SlackSettings({ setToast }: SlackSettingsProps) {
  const [settings, setSettings] = useState<Settings>(BLANK);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<string | null>(null);

  useEffect(() => {
    api
      .getSlackSettings()
      .then(setSettings)
      .catch((err) => setToast({ message: errorMessage(err), type: "error" }))
      .finally(() => setLoading(false));
  }, [setToast]);

  const save = async () => {
    setSaving(true);
    setResult(null);
    try {
      await api.saveSlackSettings(settings);
      setToast({ message: "Slack settings saved.", type: "success" });
    } catch (err) {
      setToast({ message: errorMessage(err), type: "error" });
    } finally {
      setSaving(false);
    }
  };

  const test = async () => {
    setTesting(true);
    setResult(null);
    try {
      // Saved first so the test cannot pass against a token that is no longer
      // on screen, or fail against one that has not been stored yet.
      await api.saveSlackSettings(settings);
      setResult(await api.testSlackConnection());
    } catch (err) {
      setToast({ message: errorMessage(err), type: "error" });
    } finally {
      setTesting(false);
    }
  };

  if (loading) {
    return (
      <div className="flex h-32 items-center justify-center">
        <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
      </div>
    );
  }

  const off = !settings.enabled;

  return (
    <div className="space-y-6">
      <div>
        <h3 className="flex items-center gap-2 text-lg font-medium">
          <MessageSquare className="h-4 w-4" />
          Slack
        </h3>
        <p className="text-sm text-muted-foreground">
          When an agent stops to ask something, the question goes to Slack and
          the reply in its thread is the answer — so a run can be unblocked
          without opcode in front of you. While this is on, the question is not
          also raised in the app.
        </p>
      </div>

      <div className="space-y-4">
        <div className="flex items-center justify-between">
          <div className="space-y-0.5">
            <Label htmlFor="slack-enabled">Send questions to Slack</Label>
            <p className="text-sm text-muted-foreground">
              Off means every question waits in the app instead.
            </p>
          </div>
          <Switch
            id="slack-enabled"
            checked={settings.enabled}
            onCheckedChange={(enabled) => setSettings((s) => ({ ...s, enabled }))}
          />
        </div>

        <div className="space-y-4" style={{ opacity: off ? 0.5 : 1 }}>
          <div className="space-y-2">
            <Label htmlFor="slack-token">Bot token</Label>
            <Input
              id="slack-token"
              type="password"
              autoComplete="off"
              placeholder="xoxb-…"
              value={settings.bot_token}
              onChange={(e) =>
                setSettings((s) => ({ ...s, bot_token: e.target.value }))
              }
              disabled={off}
            />
            <p className="text-xs text-muted-foreground">
              From your Slack app's OAuth page. Create the app from{" "}
              <code>templates/slack-app-manifest.json</code> and every scope it
              needs comes with it — posting, reading the reply back, and
              creating the channel.
            </p>
          </div>

          <div className="space-y-2">
            <Label htmlFor="slack-channel">Channel</Label>
            <Input
              id="slack-channel"
              placeholder="dotsquares-ai"
              value={settings.channel}
              onChange={(e) =>
                setSettings((s) => ({ ...s, channel: e.target.value }))
              }
              disabled={off}
            />
            <p className="text-xs text-muted-foreground">
              A channel name, with or without the <code>#</code>, or a channel
              id. It does not have to exist — a public channel by this name is
              created, and joined, on the first message.
            </p>
          </div>

          <div className="flex items-center justify-between">
            <div className="space-y-0.5">
              <Label htmlFor="slack-notify-runs">Post run outcomes</Label>
              <p className="text-sm text-muted-foreground">
                A line in the channel when a run finishes, fails or is stopped.
              </p>
            </div>
            <Switch
              id="slack-notify-runs"
              checked={settings.notify_runs}
              onCheckedChange={(notify_runs) =>
                setSettings((s) => ({ ...s, notify_runs }))
              }
              disabled={off}
            />
          </div>
        </div>

        <div className="flex items-center gap-2 pt-2">
          <Button onClick={save} disabled={saving || testing}>
            {saving && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
            Save
          </Button>
          <Button
            variant="outline"
            onClick={test}
            disabled={off || testing || saving || !settings.bot_token.trim()}
            className="gap-2"
          >
            {testing ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <Send className="h-4 w-4" />
            )}
            Save and send a test message
          </Button>
        </div>

        {result && (
          <p className="whitespace-pre-line rounded-md border border-green-500/40 bg-green-500/10 px-3 py-2 text-sm text-green-600 dark:text-green-400">
            {result}
          </p>
        )}
      </div>
    </div>
  );
}
