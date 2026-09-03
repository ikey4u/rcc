#include <exception>
#include <iostream>
#include <stdexcept>
#include <string>
#include <thread>

static int caught = 0;

static void worker() {
    try {
        throw std::runtime_error("rcc-cxx");
    } catch (const std::runtime_error &error) {
        if (std::string(error.what()) == "rcc-cxx") {
            caught = 1;
        }
    }
}

int main() {
    std::thread thread(worker);
    thread.join();
    if (caught != 1) {
        std::cerr << "exception path failed\n";
        return 1;
    }
    std::cout << "rcc-cxx-ok iostream thread exception\n";
    return 0;
}
