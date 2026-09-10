import React, { useState, useEffect } from "react";
import { motion, AnimatePresence } from "framer-motion";
import {
  Plus,
  Trash2,
  Edit,
  Save,
  Sparkles,
  Globe,
  FolderOpen,
  Paperclip,
  AlertCircle,
  AlertTriangle,
  Loader2,
  Search,
  ChevronDown,
  ChevronRight,
  Code
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { Card } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from "@/components/ui/dialog";
import { api, type Skill } from "@/lib/api";
import { cn } from "@/lib/utils";

// Plain tool names for a skill's `allowed-tools` frontmatter. Deliberately
// not the hooks' COMMON_TOOL_MATCHERS list — those are regex matchers
// ("Edit|Write", "mcp__.*") which aren't valid here.
const SKILL_TOOLS = [
  "Task",
  "Bash",
  "Glob",
  "Grep",
  "Read",
  "Edit",
  "Write",
  "NotebookEdit",
  "WebFetch",
  "WebSearch"
];

interface SkillsManagerProps {
  projectPath?: string;
  className?: string;
  scopeFilter?: 'project' | 'user' | 'all';
}

interface SkillForm {
  name: string;
  description: string;
  content: string;
  allowedTools: string[];
  scope: 'project' | 'user';
}

const EXAMPLE_SKILLS = [
  {
    name: "code-review",
    description:
      "Review changed code for correctness bugs and simplifications. Use when the user asks for a review of their diff, branch, or a specific file.",
    content:
      "## Steps\n\n1. Run `git diff` to see what changed.\n2. Read each changed file in full for context — not just the diff hunks.\n3. Report findings most-severe first, each with a concrete failure scenario.\n\n## Rules\n\n- Only report issues you can trace to a specific line.\n- Skip style nits unless they hide a real bug.",
    allowedTools: ["Read", "Grep", "Bash"]
  },
  {
    name: "release-notes",
    description:
      "Draft release notes from recent commits. Use when the user asks for a changelog, release notes, or a summary of what shipped.",
    content:
      "## Steps\n\n1. Run `git log --oneline` since the last tag.\n2. Group commits into Features / Fixes / Internal.\n3. Write one user-facing line per entry — what changed for the user, not the implementation.",
    allowedTools: ["Bash", "Read", "Write"]
  },
  {
    name: "run-tests",
    description:
      "Run this project's test suite and triage failures. Use when the user asks to run tests, check if things pass, or fix failing tests.",
    content:
      "## Steps\n\n1. Detect the runner from package.json / Cargo.toml.\n2. Run the suite and capture the output.\n3. For each failure, read the failing test and the code under test before proposing a fix.",
    allowedTools: ["Bash", "Read", "Edit"]
  },
  {
    name: "debug-issue",
    description:
      "Systematically debug a reported bug. Use when the user reports something broken, crashing, or behaving unexpectedly.",
    content:
      "## Steps\n\n1. Reproduce the issue before changing anything.\n2. Form a hypothesis and confirm it with logs or a narrow test.\n3. Fix the root cause, not the symptom, then verify the reproduction is gone.",
    allowedTools: ["Read", "Grep", "Bash", "Edit"]
  }
];

/**
 * SkillsManager component for managing agent skills.
 *
 * Skills live at `.claude/skills/<name>/SKILL.md` (project scope) or
 * `~/.claude/skills/<name>/SKILL.md` (user scope) — the same layout the
 * Claude Code CLI discovers, so anything created here is immediately
 * invocable as `/<name>` in a session.
 */
export const SkillsManager: React.FC<SkillsManagerProps> = ({
  projectPath,
  className,
  scopeFilter = 'all',
}) => {
  const [skills, setSkills] = useState<Skill[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState("");
  const [selectedScope, setSelectedScope] = useState<'all' | 'project' | 'user'>(
    scopeFilter === 'all' ? 'all' : (scopeFilter as 'project' | 'user')
  );
  const [expandedSkills, setExpandedSkills] = useState<Set<string>>(new Set());

  // Edit dialog state
  const [editDialogOpen, setEditDialogOpen] = useState(false);
  const [editingSkill, setEditingSkill] = useState<Skill | null>(null);
  const [skillForm, setSkillForm] = useState<SkillForm>({
    name: "",
    description: "",
    content: "",
    allowedTools: [],
    scope: 'user'
  });

  // Delete confirmation dialog state
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false);
  const [skillToDelete, setSkillToDelete] = useState<Skill | null>(null);
  const [deleting, setDeleting] = useState(false);

  useEffect(() => {
    loadSkills();
  }, [projectPath]);

  const loadSkills = async () => {
    try {
      setLoading(true);
      setError(null);
      const loadedSkills = await api.skillsList(projectPath);
      setSkills(loadedSkills);
    } catch (err) {
      console.error("Failed to load skills:", err);
      setError("Failed to load skills");
    } finally {
      setLoading(false);
    }
  };

  const handleCreateNew = () => {
    setEditingSkill(null);
    setSkillForm({
      name: "",
      description: "",
      content: "",
      allowedTools: [],
      scope: scopeFilter !== 'all' ? scopeFilter : (projectPath ? 'project' : 'user')
    });
    setEditDialogOpen(true);
  };

  const handleEdit = (skill: Skill) => {
    setEditingSkill(skill);
    setSkillForm({
      name: skill.name,
      description: skill.description || "",
      content: skill.content,
      allowedTools: skill.allowed_tools,
      scope: skill.scope as 'project' | 'user'
    });
    setEditDialogOpen(true);
  };

  const handleSave = async () => {
    try {
      setSaving(true);
      setError(null);

      await api.skillSave(
        skillForm.scope,
        skillForm.name.trim(),
        skillForm.description,
        skillForm.content,
        skillForm.allowedTools,
        skillForm.scope === 'project' ? projectPath : undefined
      );

      setEditDialogOpen(false);
      await loadSkills();
    } catch (err) {
      console.error("Failed to save skill:", err);
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  const handleDeleteClick = (skill: Skill) => {
    setSkillToDelete(skill);
    setDeleteDialogOpen(true);
  };

  const confirmDelete = async () => {
    if (!skillToDelete) return;

    try {
      setDeleting(true);
      setError(null);
      await api.skillDelete(skillToDelete.id, projectPath);
      setDeleteDialogOpen(false);
      setSkillToDelete(null);
      await loadSkills();
    } catch (err) {
      console.error("Failed to delete skill:", err);
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setDeleting(false);
    }
  };

  const cancelDelete = () => {
    setDeleteDialogOpen(false);
    setSkillToDelete(null);
  };

  const toggleExpanded = (skillId: string) => {
    setExpandedSkills(prev => {
      const next = new Set(prev);
      if (next.has(skillId)) {
        next.delete(skillId);
      } else {
        next.add(skillId);
      }
      return next;
    });
  };

  const handleToolToggle = (tool: string) => {
    setSkillForm(prev => ({
      ...prev,
      allowedTools: prev.allowedTools.includes(tool)
        ? prev.allowedTools.filter(t => t !== tool)
        : [...prev.allowedTools, tool]
    }));
  };

  const applyExample = (example: typeof EXAMPLE_SKILLS[0]) => {
    setSkillForm(prev => ({
      ...prev,
      name: example.name,
      description: example.description,
      content: example.content,
      allowedTools: example.allowedTools
    }));
  };

  // A skill name has to be a valid directory name the CLI can resolve
  const nameError = skillForm.name && !/^[a-z0-9][a-z0-9-]*$/.test(skillForm.name.trim())
    ? "Use lowercase letters, digits and hyphens only (e.g. code-review)"
    : null;

  const filteredSkills = skills.filter(skill => {
    if (scopeFilter !== 'all' && skill.scope !== scopeFilter) return false;
    if (selectedScope !== 'all' && skill.scope !== selectedScope) return false;

    if (searchQuery) {
      const query = searchQuery.toLowerCase();
      return (
        skill.name.toLowerCase().includes(query) ||
        (skill.description && skill.description.toLowerCase().includes(query))
      );
    }

    return true;
  });

  const groupedSkills = filteredSkills.reduce((acc, skill) => {
    const key = skill.scope === 'project' ? 'Project Skills' : 'User Skills';
    if (!acc[key]) acc[key] = [];
    acc[key].push(skill);
    return acc;
  }, {} as Record<string, Skill[]>);

  return (
    <div className={cn("space-y-4", className)}>
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h3 className="text-lg font-semibold">
            {scopeFilter === 'project' ? 'Project Skills' : 'Skills'}
          </h3>
          <p className="text-sm text-muted-foreground mt-1">
            Reusable instructions Claude loads when a task matches — invoke with <code>/name</code>
          </p>
        </div>
        <Button onClick={handleCreateNew} size="sm" className="gap-2">
          <Plus className="h-4 w-4" />
          New Skill
        </Button>
      </div>

      {/* Filters */}
      <div className="flex items-center gap-4">
        <div className="flex-1">
          <div className="relative">
            <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 h-4 w-4 text-muted-foreground" />
            <Input
              placeholder="Search skills..."
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              className="pl-9"
            />
          </div>
        </div>
        {scopeFilter === 'all' && (
          <Select value={selectedScope} onValueChange={(value: any) => setSelectedScope(value)}>
            <SelectTrigger className="w-[150px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All Skills</SelectItem>
              <SelectItem value="project">Project</SelectItem>
              <SelectItem value="user">User</SelectItem>
            </SelectContent>
          </Select>
        )}
      </div>

      {/* Error Message */}
      {error && (
        <div className="flex items-center gap-2 p-3 rounded-lg bg-destructive/10 text-destructive">
          <AlertCircle className="h-4 w-4 flex-shrink-0" />
          <span className="text-sm">{error}</span>
        </div>
      )}

      {/* Skills List */}
      {loading ? (
        <div className="flex items-center justify-center py-8">
          <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
        </div>
      ) : filteredSkills.length === 0 ? (
        <Card className="p-8">
          <div className="text-center">
            <Sparkles className="h-12 w-12 mx-auto text-muted-foreground mb-4" />
            <p className="text-sm text-muted-foreground">
              {searchQuery ? "No skills found" : "No skills created yet"}
            </p>
            {!searchQuery && (
              <Button onClick={handleCreateNew} variant="outline" size="sm" className="mt-4">
                Create your first skill
              </Button>
            )}
          </div>
        </Card>
      ) : (
        <div className="space-y-4">
          {Object.entries(groupedSkills).map(([groupKey, groupSkills]) => (
            <Card key={groupKey} className="overflow-hidden">
              <div className="p-4 bg-muted/50 border-b">
                <h4 className="text-sm font-medium">{groupKey}</h4>
              </div>

              <div className="divide-y">
                {groupSkills.map((skill) => {
                  const isExpanded = expandedSkills.has(skill.id);
                  const ScopeIcon = skill.scope === 'project' ? FolderOpen : Globe;

                  return (
                    <div key={skill.id} className="p-4">
                      <div className="flex items-start gap-4">
                        <ScopeIcon className="h-5 w-5 mt-0.5 text-muted-foreground flex-shrink-0" />

                        <div className="flex-1 min-w-0">
                          <div className="flex items-center gap-2 mb-1">
                            <code className="text-sm font-mono text-primary">/{skill.name}</code>
                            {skill.supporting_files.length > 0 && (
                              <Badge variant="secondary" className="text-xs gap-1">
                                <Paperclip className="h-3 w-3" />
                                {skill.supporting_files.length} file
                                {skill.supporting_files.length === 1 ? '' : 's'}
                              </Badge>
                            )}
                          </div>

                          {skill.description ? (
                            <p className="text-sm text-muted-foreground mb-2">{skill.description}</p>
                          ) : (
                            <p className="text-sm text-amber-500 mb-2 flex items-center gap-1.5">
                              <AlertTriangle className="h-3.5 w-3.5 flex-shrink-0" />
                              No description — Claude can't tell when this skill applies
                            </p>
                          )}

                          {skill.frontmatter_name && (
                            <p className="text-xs text-amber-500 mb-2 flex items-center gap-1.5">
                              <AlertTriangle className="h-3.5 w-3.5 flex-shrink-0" />
                              Frontmatter says <code>{skill.frontmatter_name}</code>, but it's
                              invoked as <code>/{skill.name}</code> (the directory name wins)
                            </p>
                          )}

                          <div className="flex items-center gap-4 text-xs">
                            {skill.allowed_tools.length > 0 && (
                              <span className="text-muted-foreground">
                                {skill.allowed_tools.length} tool
                                {skill.allowed_tools.length === 1 ? '' : 's'}
                              </span>
                            )}

                            <button
                              onClick={() => toggleExpanded(skill.id)}
                              className="flex items-center gap-1 text-muted-foreground hover:text-foreground transition-colors"
                            >
                              {isExpanded ? (
                                <>
                                  <ChevronDown className="h-3 w-3" />
                                  Hide instructions
                                </>
                              ) : (
                                <>
                                  <ChevronRight className="h-3 w-3" />
                                  Show instructions
                                </>
                              )}
                            </button>
                          </div>
                        </div>

                        <div className="flex items-center gap-2">
                          <Button
                            variant="ghost"
                            size="icon"
                            onClick={() => handleEdit(skill)}
                            className="h-8 w-8"
                          >
                            <Edit className="h-4 w-4" />
                          </Button>
                          <Button
                            variant="ghost"
                            size="icon"
                            onClick={() => handleDeleteClick(skill)}
                            className="h-8 w-8 text-destructive hover:text-destructive"
                          >
                            <Trash2 className="h-4 w-4" />
                          </Button>
                        </div>
                      </div>

                      <AnimatePresence>
                        {isExpanded && (
                          <motion.div
                            initial={{ height: 0, opacity: 0 }}
                            animate={{ height: "auto", opacity: 1 }}
                            exit={{ height: 0, opacity: 0 }}
                            transition={{ duration: 0.2 }}
                            className="overflow-hidden"
                          >
                            <div className="mt-4 p-3 bg-muted/50 rounded-md">
                              <pre className="text-xs font-mono whitespace-pre-wrap">
                                {skill.content}
                              </pre>
                            </div>
                            {skill.supporting_files.length > 0 && (
                              <div className="mt-2 p-3 bg-muted/30 rounded-md">
                                <p className="text-xs font-medium text-muted-foreground mb-1">
                                  Supporting files
                                </p>
                                <ul className="space-y-0.5">
                                  {skill.supporting_files.map((file) => (
                                    <li key={file} className="text-xs font-mono text-muted-foreground">
                                      {file}
                                    </li>
                                  ))}
                                </ul>
                              </div>
                            )}
                            <p className="mt-2 text-xs font-mono text-muted-foreground truncate">
                              {skill.file_path}
                            </p>
                          </motion.div>
                        )}
                      </AnimatePresence>
                    </div>
                  );
                })}
              </div>
            </Card>
          ))}
        </div>
      )}

      {/* Edit Dialog */}
      <Dialog open={editDialogOpen} onOpenChange={setEditDialogOpen}>
        <DialogContent className="max-w-4xl max-h-[90vh] overflow-y-auto">
          <DialogHeader>
            <DialogTitle>{editingSkill ? "Edit Skill" : "Create New Skill"}</DialogTitle>
          </DialogHeader>

          <div className="space-y-4 py-4">
            {/* Scope */}
            <div className="space-y-2">
              <Label>Scope</Label>
              <Select
                value={skillForm.scope}
                onValueChange={(value: 'project' | 'user') =>
                  setSkillForm(prev => ({ ...prev, scope: value }))
                }
                disabled={!!editingSkill || scopeFilter !== 'all'}
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {(scopeFilter === 'all' || scopeFilter === 'user') && (
                    <SelectItem value="user">
                      <div className="flex items-center gap-2">
                        <Globe className="h-4 w-4" />
                        User (Global)
                      </div>
                    </SelectItem>
                  )}
                  {(scopeFilter === 'all' || scopeFilter === 'project') && (
                    <SelectItem value="project" disabled={!projectPath}>
                      <div className="flex items-center gap-2">
                        <FolderOpen className="h-4 w-4" />
                        Project
                      </div>
                    </SelectItem>
                  )}
                </SelectContent>
              </Select>
              <p className="text-xs text-muted-foreground">
                {skillForm.scope === 'user'
                  ? "Stored in ~/.claude/skills — available in every project"
                  : "Stored in this project's .claude/skills — committable, shared with the team"}
              </p>
            </div>

            {/* Name */}
            <div className="space-y-2">
              <Label>Skill Name*</Label>
              <Input
                placeholder="e.g., code-review"
                value={skillForm.name}
                onChange={(e) => setSkillForm(prev => ({ ...prev, name: e.target.value }))}
                disabled={!!editingSkill}
              />
              {nameError ? (
                <p className="text-xs text-destructive">{nameError}</p>
              ) : (
                <p className="text-xs text-muted-foreground">
                  {editingSkill
                    ? "Renaming isn't supported — delete and recreate to change the name"
                    : "Becomes the directory name and the /command used to invoke it"}
                </p>
              )}
            </div>

            {/* Description */}
            <div className="space-y-2">
              <Label>Description*</Label>
              <Textarea
                placeholder="What it does and when to use it — e.g. 'Review changed code for bugs. Use when the user asks for a review of their diff or branch.'"
                value={skillForm.description}
                onChange={(e) => setSkillForm(prev => ({ ...prev, description: e.target.value }))}
                className="min-h-[70px] text-sm"
              />
              <p className="text-xs text-muted-foreground">
                This is the only part Claude reads before deciding to load the skill, so say
                <em> when </em> to use it, not just what it does.
              </p>
            </div>

            {/* Instructions */}
            <div className="space-y-2">
              <Label>Instructions*</Label>
              <Textarea
                placeholder={"## Steps\n\n1. ...\n2. ...\n\n## Rules\n\n- ..."}
                value={skillForm.content}
                onChange={(e) => setSkillForm(prev => ({ ...prev, content: e.target.value }))}
                className="min-h-[200px] font-mono text-sm"
              />
              <p className="text-xs text-muted-foreground">
                Markdown instructions Claude follows once the skill loads. Concrete steps and file
                paths beat general advice.
              </p>
            </div>

            {/* Allowed Tools */}
            <div className="space-y-2">
              <Label>Allowed Tools (Optional)</Label>
              <div className="flex flex-wrap gap-2">
                {SKILL_TOOLS.map((tool) => (
                  <Button
                    key={tool}
                    variant={skillForm.allowedTools.includes(tool) ? "default" : "outline"}
                    size="sm"
                    onClick={() => handleToolToggle(tool)}
                    type="button"
                  >
                    {tool}
                  </Button>
                ))}
              </div>
              <p className="text-xs text-muted-foreground">
                Leave empty to let the skill use whatever the session already allows
              </p>
            </div>

            {/* Examples */}
            {!editingSkill && (
              <div className="space-y-2">
                <Label>Start from an example</Label>
                <div className="grid grid-cols-2 gap-2">
                  {EXAMPLE_SKILLS.map((example) => (
                    <Button
                      key={example.name}
                      variant="outline"
                      size="sm"
                      onClick={() => applyExample(example)}
                      className="justify-start"
                    >
                      <Code className="h-4 w-4 mr-2 flex-shrink-0" />
                      {example.name}
                    </Button>
                  ))}
                </div>
              </div>
            )}

            {/* Preview */}
            {skillForm.name && !nameError && (
              <div className="space-y-2">
                <Label>Preview</Label>
                <div className="p-3 bg-muted rounded-md space-y-1">
                  <code className="text-sm">/{skillForm.name.trim()}</code>
                  <p className="text-xs font-mono text-muted-foreground">
                    {skillForm.scope === 'project'
                      ? `${projectPath || '<project>'}/.claude/skills/${skillForm.name.trim()}/SKILL.md`
                      : `~/.claude/skills/${skillForm.name.trim()}/SKILL.md`}
                  </p>
                </div>
              </div>
            )}
          </div>

          <DialogFooter>
            <Button variant="outline" onClick={() => setEditDialogOpen(false)}>
              Cancel
            </Button>
            <Button
              onClick={handleSave}
              disabled={
                !skillForm.name.trim() ||
                !skillForm.description.trim() ||
                !skillForm.content.trim() ||
                !!nameError ||
                saving
              }
            >
              {saving ? (
                <>
                  <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                  Saving...
                </>
              ) : (
                <>
                  <Save className="h-4 w-4 mr-2" />
                  Save
                </>
              )}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Delete Confirmation Dialog */}
      <Dialog open={deleteDialogOpen} onOpenChange={setDeleteDialogOpen}>
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>Delete Skill</DialogTitle>
          </DialogHeader>

          <div className="space-y-4 py-4">
            <p>Are you sure you want to delete this skill?</p>
            {skillToDelete && (
              <div className="p-3 bg-muted rounded-md">
                <code className="text-sm font-mono">/{skillToDelete.name}</code>
                {skillToDelete.description && (
                  <p className="text-sm text-muted-foreground mt-1">{skillToDelete.description}</p>
                )}
                <p className="text-xs font-mono text-muted-foreground mt-2 break-all">
                  {skillToDelete.directory_path}
                </p>
              </div>
            )}
            <p className="text-sm text-muted-foreground">
              This cannot be undone. The whole skill directory is deleted
              {skillToDelete && skillToDelete.supporting_files.length > 0
                ? `, including its ${skillToDelete.supporting_files.length} supporting file${
                    skillToDelete.supporting_files.length === 1 ? '' : 's'
                  }.`
                : '.'}
            </p>
          </div>

          <DialogFooter>
            <Button variant="outline" onClick={cancelDelete} disabled={deleting}>
              Cancel
            </Button>
            <Button variant="destructive" onClick={confirmDelete} disabled={deleting}>
              {deleting ? (
                <>
                  <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                  Deleting...
                </>
              ) : (
                <>
                  <Trash2 className="h-4 w-4 mr-2" />
                  Delete
                </>
              )}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
};
