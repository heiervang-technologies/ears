// Ears Ghost: an fcitx5 module that lets the ears speech daemon show
// in-progress transcription as inline preedit ("ghost text") in whichever
// application currently has input focus, and commit it when final.
//
// Protocol: ears connects to $XDG_RUNTIME_DIR/ears/ghost.sock and sends
// newline-terminated commands. Text is escaped: "\\" -> backslash,
// "\n" -> newline.
//
//   P <text>   show <text> as preedit (replaces any previous ghost)
//   C <text>   clear the ghost and commit <text> to the application
//   X          clear the ghost without committing
//   S          status query
//
// Every command gets one reply line:
//   OK preedit|panel|none   (how the ghost is being displayed / was sent)
//   ERR <reason>
//
// The ghost is bound to the input context it was first shown in. If focus
// moves elsewhere, the old context's ghost is cleared and later text goes
// to the newly focused context, so text never lands in a window the user
// has left mid-utterance without the next update following them.

#include <fcitx-utils/event.h>
#include <fcitx-utils/log.h>
#include <fcitx-utils/trackableobject.h>
#include <fcitx/addonfactory.h>
#include <fcitx/addoninstance.h>
#include <fcitx/addonmanager.h>
#include <fcitx/inputcontext.h>
#include <fcitx/inputpanel.h>
#include <fcitx/instance.h>
#include <fcitx/text.h>
#include <fcitx/userinterface.h>

#include <cerrno>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <map>
#include <memory>
#include <string>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>

FCITX_DEFINE_LOG_CATEGORY(ears_ghost, "ears-ghost");
#define GHOST_INFO() FCITX_LOGC(ears_ghost, Info)
#define GHOST_WARN() FCITX_LOGC(ears_ghost, Warn)

namespace {

std::string socketPath() {
    const char *runtime = std::getenv("XDG_RUNTIME_DIR");
    std::string dir = runtime && *runtime
                          ? std::string(runtime)
                          : "/run/user/" + std::to_string(getuid());
    dir += "/ears";
    ::mkdir(dir.c_str(), 0700);
    return dir + "/ghost.sock";
}

std::string unescape(const std::string &in) {
    std::string out;
    out.reserve(in.size());
    for (size_t i = 0; i < in.size(); ++i) {
        if (in[i] == '\\' && i + 1 < in.size()) {
            char n = in[++i];
            out.push_back(n == 'n' ? '\n' : n);
        } else {
            out.push_back(in[i]);
        }
    }
    return out;
}

void setNonBlocking(int fd) {
    int flags = fcntl(fd, F_GETFL);
    if (flags >= 0) {
        fcntl(fd, F_SETFL, flags | O_NONBLOCK);
    }
    fcntl(fd, F_SETFD, FD_CLOEXEC);
}

} // namespace

class EarsGhost : public fcitx::AddonInstance {
public:
    explicit EarsGhost(fcitx::Instance *instance) : instance_(instance) {
        listen();
    }

    ~EarsGhost() override {
        clearGhost();
        clients_.clear();
        listenEvent_.reset();
        if (listenFd_ >= 0) {
            ::close(listenFd_);
            // Only remove the socket if it is still ours: during `fcitx5 -r`
            // the replacement instance has already bound a new one at the
            // same path, and unlinking that would orphan it.
            struct stat st {};
            if (::stat(path_.c_str(), &st) == 0 && st.st_ino == socketIno_) {
                ::unlink(path_.c_str());
            }
        }
    }

private:
    struct Client {
        int fd = -1;
        uint64_t id = 0;
        std::string buffer;
        std::unique_ptr<fcitx::EventSourceIO> event;
        ~Client() {
            event.reset();
            if (fd >= 0) {
                ::close(fd);
            }
        }
    };

    void listen() {
        path_ = socketPath();
        ::unlink(path_.c_str());
        listenFd_ = ::socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
        if (listenFd_ < 0) {
            GHOST_WARN() << "socket() failed: " << std::strerror(errno);
            return;
        }
        sockaddr_un addr{};
        addr.sun_family = AF_UNIX;
        std::strncpy(addr.sun_path, path_.c_str(), sizeof(addr.sun_path) - 1);
        if (::bind(listenFd_, reinterpret_cast<sockaddr *>(&addr),
                   sizeof(addr)) < 0 ||
            ::listen(listenFd_, 4) < 0) {
            GHOST_WARN() << "cannot listen on " << path_ << ": "
                         << std::strerror(errno);
            ::close(listenFd_);
            listenFd_ = -1;
            return;
        }
        ::chmod(path_.c_str(), 0600);
        struct stat st {};
        if (::stat(path_.c_str(), &st) == 0) {
            socketIno_ = st.st_ino;
        }
        setNonBlocking(listenFd_);
        listenEvent_ = instance_->eventLoop().addIOEvent(
            listenFd_, fcitx::IOEventFlag::In,
            [this](fcitx::EventSourceIO *, int, fcitx::IOEventFlags) {
                accept();
                return true;
            });
        GHOST_INFO() << "listening on " << path_;
    }

    void accept() {
        for (;;) {
            int fd = ::accept(listenFd_, nullptr, nullptr);
            if (fd < 0) {
                return;
            }
            setNonBlocking(fd);
            auto client = std::make_unique<Client>();
            client->fd = fd;
            client->id = ++nextClientId_;
            client->event = instance_->eventLoop().addIOEvent(
                fd, fcitx::IOEventFlag::In,
                [this](fcitx::EventSourceIO *, int cfd, fcitx::IOEventFlags) {
                    readClient(cfd);
                    return true;
                });
            clients_[fd] = std::move(client);
        }
    }

    void dropClient(int fd) {
        // Scheduled: we may be inside this client's own callback.
        instance_->eventLoop()
            .addDeferEvent([this, fd](fcitx::EventSource *) {
                clients_.erase(fd);
                return false;
            })
            .release();
    }

    void readClient(int fd) {
        auto it = clients_.find(fd);
        if (it == clients_.end()) {
            return;
        }
        Client &client = *it->second;
        char buf[4096];
        for (;;) {
            ssize_t n = ::read(fd, buf, sizeof(buf));
            if (n > 0) {
                client.buffer.append(buf, static_cast<size_t>(n));
                if (client.buffer.size() > (1 << 20)) {
                    GHOST_WARN() << "client line too long, dropping";
                    dropClient(fd);
                    return;
                }
                continue;
            }
            if (n == 0 || (errno != EAGAIN && errno != EWOULDBLOCK &&
                           errno != EINTR)) {
                // Peer went away: a dead ears must not leave ghost text.
                // Only its own ghost, though: a status probe or an older
                // preview disconnecting must not erase a newer one.
                handleLines(client);
                if (client.id == ownerId_) {
                    clearGhost();
                }
                dropClient(fd);
                return;
            }
            if (errno == EINTR) {
                continue;
            }
            break;
        }
        handleLines(client);
    }

    void handleLines(Client &client) {
        size_t pos;
        while ((pos = client.buffer.find('\n')) != std::string::npos) {
            std::string line = client.buffer.substr(0, pos);
            client.buffer.erase(0, pos + 1);
            std::string reply = handle(client.id, line) + "\n";
            // Best effort; replies are advisory.
            (void)!::write(client.fd, reply.data(), reply.size());
        }
    }

    /// The input context that has keyboard focus right now, or nullptr.
    ///
    /// lastFocusedInputContext() keeps pointing at the previous app after
    /// focus moves to a surface that never opens an input method (some
    /// browser fields, XWayland apps). Committing there would put the text
    /// into a window the user left, so require real focus; ears types the
    /// text instead when there is none.
    fcitx::InputContext *focusedNow() {
        auto *ic = instance_->lastFocusedInputContext();
        return ic && ic->hasFocus() ? ic : nullptr;
    }

    fcitx::InputContext *target() {
        auto *focused = focusedNow();
        auto *current = ghostIc_.get();
        if (current && current != focused) {
            // Focus moved: take the ghost out of the window we left.
            clearOn(current);
            ghostIc_.unwatch();
            ownerId_ = 0;
        }
        return focused;
    }

    std::string mode(fcitx::InputContext *ic) {
        return ic->capabilityFlags().test(fcitx::CapabilityFlag::Preedit)
                   ? "preedit"
                   : "panel";
    }

    std::string handle(uint64_t clientId, const std::string &line) {
        if (line.empty()) {
            return "ERR empty";
        }
        char op = line[0];
        std::string text =
            line.size() > 2 ? unescape(line.substr(2)) : std::string();
        switch (op) {
        case 'P': {
            auto *ic = target();
            if (!ic) {
                return "OK none";
            }
            showOn(ic, text);
            ghostIc_ = ic->watch();
            ownerId_ = clientId;
            return "OK " + mode(ic);
        }
        case 'C': {
            auto *ic = target();
            if (!ic) {
                return "OK none";
            }
            clearOn(ic);
            ghostIc_.unwatch();
            ownerId_ = 0;
            if (!text.empty()) {
                ic->commitString(text);
            }
            return "OK " + mode(ic);
        }
        case 'X':
            // Only the ghost's owner may clear it.
            if (clientId == ownerId_) {
                clearGhost();
            }
            return "OK none";
        case 'S': {
            auto *ic = focusedNow();
            return ic ? "OK " + mode(ic) + " " + ic->program()
                      : std::string("OK none");
        }
        default:
            return "ERR unknown command";
        }
    }

    void showOn(fcitx::InputContext *ic, const std::string &text) {
        fcitx::Text t;
        t.append(text, fcitx::TextFormatFlag::Underline);
        // Ghost semantics: the caret stays where the user is; the suggested
        // text trails after it instead of pushing the cursor along.
        t.setCursor(0);
        if (ic->capabilityFlags().test(fcitx::CapabilityFlag::Preedit)) {
            ic->inputPanel().setClientPreedit(t);
        } else {
            // App cannot draw preedit: fcitx shows it in its own panel.
            ic->inputPanel().setPreedit(t);
        }
        ic->updatePreedit();
        ic->updateUserInterface(fcitx::UserInterfaceComponent::InputPanel);
    }

    void clearOn(fcitx::InputContext *ic) {
        ic->inputPanel().setClientPreedit(fcitx::Text());
        ic->inputPanel().setPreedit(fcitx::Text());
        ic->updatePreedit();
        ic->updateUserInterface(fcitx::UserInterfaceComponent::InputPanel);
    }

    void clearGhost() {
        if (auto *ic = ghostIc_.get()) {
            clearOn(ic);
        }
        ghostIc_.unwatch();
        ownerId_ = 0;
    }

    fcitx::Instance *instance_;
    std::string path_;
    int listenFd_ = -1;
    ino_t socketIno_ = 0;
    std::unique_ptr<fcitx::EventSourceIO> listenEvent_;
    std::map<int, std::unique_ptr<Client>> clients_;
    fcitx::TrackableObjectReference<fcitx::InputContext> ghostIc_;
    // Client whose P put the ghost up; 0 when none is showing.
    uint64_t ownerId_ = 0;
    uint64_t nextClientId_ = 0;
};

class EarsGhostFactory : public fcitx::AddonFactory {
public:
    fcitx::AddonInstance *create(fcitx::AddonManager *manager) override {
        return new EarsGhost(manager->instance());
    }
};

FCITX_ADDON_FACTORY(EarsGhostFactory);
