# vim:ft=zsh ts=2 sw=2 sts=2
#
# agnoster's Theme - https://gist.github.com/3712874
# A Powerline-inspired theme for ZSH
#
# # README
#
# In order for this theme to render correctly, you will need a
# [Powerline-patched font](https://github.com/Lokaltog/powerline-fonts).
# Make sure you have a recent version: the code points that Powerline
# uses changed in 2012, and older versions will display incorrectly,
# in confusing ways.
#
# In addition, I recommend the
# [Solarized theme](https://github.com/altercation/solarized/) and, if you're
# using it on Mac OS X, [iTerm 2](https://iterm2.com/) over Terminal.app -
# it has significantly better color fidelity.
#
# If using with "light" variant of the Solarized color schema, set
# SOLARIZED_THEME variable to "light". If you don't specify, we'll assume
# you're using the "dark" variant.
#
# # Goals
#
# The aim of this theme is to only show you *relevant* information. Like most
# prompts, it will only show git information when in a git working directory.
# However, it goes a step further: everything from the current user and
# hostname to whether the last call exited with an error to whether background
# jobs are running in this shell will all be displayed automatically when
# appropriate.

### Segment drawing
# A few utility functions to make it easy and re-usable to draw segmented prompts

CURRENT_BG='NONE'

case ${SOLARIZED_THEME:-dark} in
    light)
      CURRENT_FG=${CURRENT_FG:-'white'}
      CURRENT_DEFAULT_FG=${CURRENT_DEFAULT_FG:-'white'}
      ;;
    *)
      CURRENT_FG=${CURRENT_FG:-'black'}
      CURRENT_DEFAULT_FG=${CURRENT_DEFAULT_FG:-'default'}
      ;;
esac

### Theme Configuration Initialization
#
# Override these settings in your ~/.zshrc

# Current working directory
: ${AGNOSTER_DIR_FG:=${CURRENT_FG}}
: ${AGNOSTER_DIR_BG:=blue}

# user@host
: ${AGNOSTER_CONTEXT_FG:=${CURRENT_DEFAULT_FG}}
: ${AGNOSTER_CONTEXT_BG:=black}

# Git related
: ${AGNOSTER_GIT_CLEAN_FG:=${CURRENT_FG}}
: ${AGNOSTER_GIT_CLEAN_BG:=green}
: ${AGNOSTER_GIT_DIRTY_FG:=black}
: ${AGNOSTER_GIT_DIRTY_BG:=yellow}

# Bazaar related
: ${AGNOSTER_BZR_CLEAN_FG:=${CURRENT_FG}}
: ${AGNOSTER_BZR_CLEAN_BG:=green}
: ${AGNOSTER_BZR_DIRTY_FG:=black}
: ${AGNOSTER_BZR_DIRTY_BG:=yellow}

# Mercurial related
: ${AGNOSTER_HG_NEWFILE_FG:=white}
: ${AGNOSTER_HG_NEWFILE_BG:=red}
: ${AGNOSTER_HG_CHANGED_FG:=black}
: ${AGNOSTER_HG_CHANGED_BG:=yellow}
: ${AGNOSTER_HG_CLEAN_FG:=${CURRENT_FG}}
: ${AGNOSTER_HG_CLEAN_BG:=green}

# VirtualEnv colors
: ${AGNOSTER_VENV_FG:=black}
: ${AGNOSTER_VENV_BG:=blue}

# AWS Profile colors
: ${AGNOSTER_AWS_PROD_FG:=yellow}
: ${AGNOSTER_AWS_PROD_BG:=red}
: ${AGNOSTER_AWS_FG:=black}
: ${AGNOSTER_AWS_BG:=green}

# Status symbols
: ${AGNOSTER_STATUS_RETVAL_FG:=red}
: ${AGNOSTER_STATUS_ROOT_FG:=yellow}
: ${AGNOSTER_STATUS_JOB_FG:=cyan}
: ${AGNOSTER_STATUS_FG:=${CURRENT_DEFAULT_FG}}
: ${AGNOSTER_STATUS_BG:=black}

## Non-Color settings - set to 'true' to enable
# Show the actual numeric return value rather than a cross symbol.
: ${AGNOSTER_STATUS_RETVAL_NUMERIC:=false}
# Show git working dir in the style "/git/root   master  relative/dir" instead of "/git/root/relative/dir   master"
: ${AGNOSTER_GIT_INLINE:=false}
# Show the git branch status in the prompt rather than the generic branch symbol
: ${AGNOSTER_GIT_BRANCH_STATUS:=true}


# Special Powerline characters

# AgenticOS: the terminal environment is always C.UTF-8, so use the modern
# Powerline separator directly without probing for a host locale.
SEGMENT_SEPARATOR=$'\ue0b0'

# AgenticOS: build the prompt in-process. Upstream captures build_prompt with
# $(...), but that forks for every prompt and currently crosses an unsupported
# nested zsh SIGCHLD path in the guest kernel.
AGNOSTER_PROMPT=''

# Begin a segment
# Takes two arguments, background and foreground. Both can be omitted,
# rendering default background/foreground.
prompt_segment() {
  local bg fg
  [[ -n $1 ]] && bg="%K{$1}" || bg="%k"
  [[ -n $2 ]] && fg="%F{$2}" || fg="%f"
  if [[ $CURRENT_BG != 'NONE' && $1 != $CURRENT_BG ]]; then
    AGNOSTER_PROMPT+=" %{$bg%F{$CURRENT_BG}%}$SEGMENT_SEPARATOR%{$fg%} "
  else
    AGNOSTER_PROMPT+="%{$bg%}%{$fg%} "
  fi
  CURRENT_BG=$1
  [[ -n $3 ]] && AGNOSTER_PROMPT+=$3
}

# End the prompt, closing any open segments
prompt_end() {
  if [[ -n $CURRENT_BG ]]; then
    AGNOSTER_PROMPT+=" %{%k%F{$CURRENT_BG}%}$SEGMENT_SEPARATOR"
  else
    AGNOSTER_PROMPT+="%{%k%}"
  fi
  AGNOSTER_PROMPT+="%{%f%}"
  CURRENT_BG=''
}

git_toplevel() {
	local repo_root=$(git rev-parse --show-toplevel)
	if [[ $repo_root = '' ]]; then
		# We are in a bare repo. Use git dir as root
		repo_root=$(git rev-parse --git-dir)
		if [[ $repo_root = '.' ]]; then
			repo_root=$PWD
		fi
	fi
	echo -n $repo_root
}

### Prompt components
# Each component will draw itself, and hide itself if no information needs to be shown

# Context: user@hostname (who am I and where am I)
prompt_context() {
  if [[ "$USERNAME" != "$DEFAULT_USER" || -n "$SSH_CLIENT" ]]; then
    prompt_segment "$AGNOSTER_CONTEXT_BG" "$AGNOSTER_CONTEXT_FG" "%(!.%{%F{$AGNOSTER_STATUS_ROOT_FG}%}.)%n@%m"
  fi
}

# Host: keep the machine identity at the far left of every prompt.
prompt_hostname() {
  prompt_segment "$AGNOSTER_CONTEXT_BG" "$AGNOSTER_CONTEXT_FG" '%m'
}

prompt_git_relative() {
  local repo_root=$(git_toplevel)
  local path_in_repo=$(pwd | sed "s/^$(echo "$repo_root" | sed 's:/:\\/:g;s/\$/\\$/g')//;s:^/::;s:/$::;")
  if [[ $path_in_repo != '' ]]; then
    prompt_segment "$AGNOSTER_DIR_BG" "$AGNOSTER_DIR_FG" "$path_in_repo"
  fi;
}

# Resolve the repository control directory without launching another Git
# process after `git status` has already established that this is a worktree.
# Linked worktrees store an absolute or worktree-relative `gitdir:` pointer in
# a .git file; ordinary repositories use a .git directory.
agenticos_git_dir() {
  local dir=$PWD marker pointer
  while true; do
    marker=$dir/.git
    if [[ -d $marker ]]; then
      REPLY=$marker
      return 0
    fi
    if [[ -f $marker ]]; then
      IFS= read -r pointer < $marker || return 1
      if [[ $pointer == 'gitdir: '* ]]; then
        REPLY=${pointer#gitdir: }
        [[ $REPLY == /* ]] || REPLY=$dir/$REPLY
        return 0
      fi
      return 1
    fi
    [[ $dir == / ]] && return 1
    dir=${dir:h}
  done
}

# Git: branch/detached head, dirty status
prompt_git() {
  # AgenticOS: stay silent unless the git builtin is on PATH (GIT.ELF).
  (( $+commands[git] )) || return
  local PL_BRANCH_CHAR
  () {
    local LC_ALL="" LC_CTYPE="en_US.UTF-8"
    PL_BRANCH_CHAR=$'\ue0a0'         # 
  }
  local ref dirty mode repo_path branch_line branch_info branch_name
  # AgenticOS: staged/unstaged flags and marker, computed inline below.
  local git_status staged unstaged git_marker

  if git_status=$(command git status --porcelain=v1 --branch 2>/dev/null); then
    agenticos_git_dir && repo_path=$REPLY
    # AgenticOS: derive dirty state and the staged/unstaged markers from a
    # single `git status --porcelain` pass. Upstream agnoster relies on
    # oh-my-zsh's parse_git_dirty (not shipped here) for the dirty color and on
    # zsh's vcs_info for the ' %u%c' markers; neither works in the guest —
    # parse_git_dirty is undefined and vcs_info's git backend hits a
    # guest-specific parse error. One git fork now replaces both, and the
    # segment still renders the green/yellow background plus ± / ✚ markers.
    # Dirty (color) counts untracked files; the markers count tracked staged
    # (✚) and unstaged (±) changes only, matching the original semantics.
    dirty=''; staged=''; unstaged=''
    local -a status_lines
    status_lines=("${(@f)git_status}")
    branch_line=$status_lines[1]
    if (( ${#status_lines} > 1 )); then
      dirty=1
      local line
      for line in "${status_lines[@]:1}"; do
        [[ ${line[1]} != ' ' && ${line[1]} != '?' ]] && staged=1
        [[ ${line[2]} != ' ' && ${line[2]} != '?' ]] && unstaged=1
      done
    fi

    # `--branch` reports branch and ahead/behind in the status header, so the
    # prompt does not need symbolic-ref plus two complete `git log` walks.
    branch_info=${branch_line#\#\# }
    branch_info=${branch_info#No commits yet on }
    branch_info=${branch_info#Initial commit on }
    if [[ $branch_info == 'HEAD (no branch)'* ]]; then
      ref="◈ $(command git describe --exact-match --tags HEAD 2> /dev/null)" || \
      ref="➦ $(command git rev-parse --short HEAD 2> /dev/null)"
    else
      branch_name=${branch_info%%...*}
      branch_name=${branch_name%% *}
      ref="refs/heads/$branch_name"
    fi
    if [[ -n $dirty ]]; then
      prompt_segment "$AGNOSTER_GIT_DIRTY_BG" "$AGNOSTER_GIT_DIRTY_FG"
    else
      prompt_segment "$AGNOSTER_GIT_CLEAN_BG" "$AGNOSTER_GIT_CLEAN_FG"
    fi

    if [[ $AGNOSTER_GIT_BRANCH_STATUS == 'true' ]]; then
      if [[ $branch_info == *'[ahead '* && $branch_info == *'behind '* ]]; then
        PL_BRANCH_CHAR=$'\u21c5'
      elif [[ $branch_info == *'[ahead '* ]]; then
        PL_BRANCH_CHAR=$'\u21b1'
      elif [[ $branch_info == *'[behind '* ]]; then
        PL_BRANCH_CHAR=$'\u21b0'
      fi
    fi

    if [[ -n $repo_path ]]; then
      if [[ -e "${repo_path}/BISECT_LOG" ]]; then
        mode=" <B>"
      elif [[ -e "${repo_path}/MERGE_HEAD" ]]; then
        mode=" >M<"
      elif [[ -e "${repo_path}/rebase" || -e "${repo_path}/rebase-apply" || -e "${repo_path}/rebase-merge" || -e "${repo_path}/../.dotest" ]]; then
        mode=" >R>"
      fi
    fi

    # AgenticOS: assemble vcs_info's ' %u%c' equivalent (unstaged ±, staged ✚).
    git_marker=''
    [[ -n $unstaged ]] && git_marker+='±'
    [[ -n $staged ]] && git_marker+='✚'
    [[ -n $git_marker ]] && git_marker=" $git_marker"
    AGNOSTER_PROMPT+="${${ref:gs/%/%%}/refs\/heads\//$PL_BRANCH_CHAR }${git_marker}${mode}"
    [[ $AGNOSTER_GIT_INLINE == 'true' ]] && prompt_git_relative
  fi
}

prompt_bzr() {
  (( $+commands[bzr] )) || return

  # Test if bzr repository in directory hierarchy
  local dir="$PWD"
  while [[ ! -d "$dir/.bzr" ]]; do
    [[ "$dir" = "/" ]] && return
    dir="${dir:h}"
  done

  local bzr_status status_mod status_all revision
  if bzr_status=$(command bzr status 2>&1); then
    status_mod=$(echo -n "$bzr_status" | head -n1 | grep "modified" | wc -m)
    status_all=$(echo -n "$bzr_status" | head -n1 | wc -m)
    revision=${$(command bzr log -r-1 --log-format line | cut -d: -f1):gs/%/%%}
    if [[ $status_mod -gt 0 ]] ; then
      prompt_segment "$AGNOSTER_BZR_DIRTY_BG" "$AGNOSTER_BZR_DIRTY_FG" "bzr@$revision ✚"
    else
      if [[ $status_all -gt 0 ]] ; then
        prompt_segment "$AGNOSTER_BZR_DIRTY_BG" "$AGNOSTER_BZR_DIRTY_FG" "bzr@$revision"
      else
        prompt_segment "$AGNOSTER_BZR_CLEAN_BG" "$AGNOSTER_BZR_CLEAN_FG" "bzr@$revision"
      fi
    fi
  fi
}

prompt_hg() {
  (( $+commands[hg] )) || return
  local rev st branch
  if $(command hg id >/dev/null 2>&1); then
    if $(command hg prompt >/dev/null 2>&1); then
      if [[ $(command hg prompt "{status|unknown}") = "?" ]]; then
        # if files are not added
        prompt_segment "$AGNOSTER_HG_NEWFILE_BG" "$AGNOSTER_HG_NEWFILE_FG"
        st='±'
      elif [[ -n $(command hg prompt "{status|modified}") ]]; then
        # if any modification
        prompt_segment "$AGNOSTER_HG_CHANGED_BG" "$AGNOSTER_HG_CHANGED_FG"
        st='±'
      else
        # if working copy is clean
        prompt_segment "$AGNOSTER_HG_CLEAN_BG" "$AGNOSTER_HG_CLEAN_FG"
      fi
      AGNOSTER_PROMPT+="${$(command hg prompt "☿ {rev}@{branch}"):gs/%/%%} $st"
    else
      st=""
      rev=$(command hg id -n 2>/dev/null | sed 's/[^-0-9]//g')
      branch=$(command hg id -b 2>/dev/null)
      if command hg st | command grep -q "^\?"; then
        prompt_segment "$AGNOSTER_HG_NEWFILE_BG" "$AGNOSTER_HG_NEWFILE_FG"
        st='±'
      elif command hg st | command grep -q "^[MA]"; then
        prompt_segment "$AGNOSTER_HG_CHANGED_BG" "$AGNOSTER_HG_CHANGED_FG"
        st='±'
      else
        prompt_segment "$AGNOSTER_HG_CLEAN_BG" "$AGNOSTER_HG_CLEAN_FG"
      fi
      AGNOSTER_PROMPT+="☿ ${rev:gs/%/%%}@${branch:gs/%/%%} $st"
    fi
  fi
}

# Dir: current working directory
prompt_dir() {
  if [[ $AGNOSTER_GIT_INLINE == 'true' ]] && $(git rev-parse --is-inside-work-tree >/dev/null 2>&1); then
    # Git repo and inline path enabled, hence only show the git root
    prompt_segment "$AGNOSTER_DIR_BG" "$AGNOSTER_DIR_FG" "$(git_toplevel | sed "s:^$HOME:~:")"
  else
    prompt_segment "$AGNOSTER_DIR_BG" "$AGNOSTER_DIR_FG" '%~'
  fi
}

# Virtualenv: current working virtualenv
prompt_virtualenv() {
  if [ -n "$CONDA_DEFAULT_ENV" ]; then
    prompt_segment magenta $CURRENT_FG "🐍 $CONDA_DEFAULT_ENV"
  fi
  if [[ -n "$VIRTUAL_ENV" && -n "$VIRTUAL_ENV_DISABLE_PROMPT" ]]; then
    prompt_segment "$AGNOSTER_VENV_BG" "$AGNOSTER_VENV_FG" "(${VIRTUAL_ENV:t:gs/%/%%})"
  fi
}

# Status:
# - was there an error
# - am I root
# - are there background jobs?
prompt_status() {
  local -a symbols

  if [[ $AGNOSTER_STATUS_RETVAL_NUMERIC == 'true' ]]; then
    [[ $RETVAL -ne 0 ]] && symbols+="%{%F{$AGNOSTER_STATUS_RETVAL_FG}%}$RETVAL"
  else
    [[ $RETVAL -ne 0 ]] && symbols+="%{%F{$AGNOSTER_STATUS_RETVAL_FG}%}✘"
  fi
  [[ $UID -eq 0 ]] && symbols+="%{%F{$AGNOSTER_STATUS_ROOT_FG}%}⚡"
  # AgenticOS: zsh/parameter exposes the live job table without a pipeline or
  # command-substitution child during every prompt redraw.
  (( ${#jobstates} > 0 )) && symbols+="%{%F{$AGNOSTER_STATUS_JOB_FG}%}⚙"

  [[ -n "$symbols" ]] && prompt_segment "$AGNOSTER_STATUS_BG" "$AGNOSTER_STATUS_FG" "$symbols"
}

#AWS Profile:
# - display current AWS_PROFILE name
# - displays yellow on red if profile name contains 'production' or
#   ends in '-prod'
# - displays black on green otherwise
prompt_aws() {
  [[ -z "$AWS_PROFILE" || "$SHOW_AWS_PROMPT" = false ]] && return
  case "$AWS_PROFILE" in
    *-prod|*production*) prompt_segment "$AGNOSTER_AWS_PROD_BG" "$AGNOSTER_AWS_PROD_FG"  "AWS: ${AWS_PROFILE:gs/%/%%}" ;;
    *) prompt_segment "$AGNOSTER_AWS_BG" "$AGNOSTER_AWS_FG" "AWS: ${AWS_PROFILE:gs/%/%%}" ;;
  esac
}

## Main prompt
build_prompt() {
  RETVAL=$?
  AGNOSTER_PROMPT=''
  CURRENT_BG='NONE'
  prompt_hostname
  prompt_status
  prompt_virtualenv
  prompt_aws
  prompt_context
  prompt_dir
  prompt_git
  prompt_bzr
  prompt_hg
  prompt_end
  PROMPT="%{%f%b%k%}${AGNOSTER_PROMPT} "
}

# AgenticOS: update PROMPT from precmd in the current shell instead of using
# upstream's PROMPT='$(build_prompt)', which forks a child for every redraw.
autoload -Uz add-zsh-hook
add-zsh-hook precmd build_prompt
build_prompt
